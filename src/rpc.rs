use std::borrow::Cow;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Weak;

use bytes::Bytes;
use tokio::sync::Mutex as AsyncMutex;

// ==== Basic RPC ====

/// Generic trait for underlying RPC connection.
pub(crate) trait RpcContext<U: RpcUserData>: std::fmt::Debug {
    fn self_as_codec(&self) -> Arc<dyn Codec>;
    fn codec(&self) -> &dyn Codec;
    fn user_data(&self) -> &U;
    fn writer(&self) -> &AsyncMutex<dyn AsyncFrameWrite>;
}

/// A trait constraint for user data type of a RPC connection.
pub trait RpcUserData: Send + Sync + 'static {}
impl<T> RpcUserData for T where T: Send + Sync + 'static {}

/// A RPC client handle which can only send RPC notifications.
///
/// `NotifyClient` is lightweight handle to the underlying RPC connection.
#[derive(Debug)]
pub struct NotifyClient<U> {
    inner: Arc<dyn RpcContext<U>>,
    tx_deferred: mpsc::Sender<DeferredDirective>,
}

/// A weak RPC client handle which can only send RPC notifications.
///
/// Should be upgraded to [`NotifyClient`] before use.
#[derive(Debug)]
pub struct WeakNotifyClient<U> {
    inner: Weak<dyn RpcContext<U>>,
}

// ==== Request Capability ====

/// A RPC client handle which can send RPC requests and notifications.
#[derive(Debug)]
pub struct Client<U> {
    inner: NotifyClient<U>,
    req: Arc<RequestContext>,
}

/// A weak RPC client handle which can send RPC requests and notifications.
///
/// Should be upgraded to [`Client`] before use.
struct WeakClient<U> {
    inner: Weak<dyn RpcContext<U>>,
    req: Weak<RequestContext>,
}

/// An awaitable response for a sent RPC request
pub struct ReceiveResponse<'a> {
    reqs: Cow<'a, Arc<RequestContext>>,
    req_id: RequestId,
    state: req_rep::ReceiveResponseState,
}

// ==== Definitions ====

/// Error type definitions
pub mod error {
    use bytes::Bytes;
    use thiserror::Error;
    use tokio::sync::mpsc::error::TrySendError;

    use crate::codec;

    use super::driver::DeferredDirective;

    #[derive(Debug, Error)]
    pub enum SendError {
        #[error("Encoding failed: {0}")]
        EncodeFailed(#[from] codec::error::EncodeError),

        /// This won't be returned calling `*_deferred` methods.
        #[error("Async IO failed: {0}")]
        AsyncIoError(#[from] std::io::Error),

        /// This won't be returned calling async methods.
        #[error("Failed to send request to background driver: {0}")]
        DeferredIoError(#[from] DeferredActionError<Bytes>),
    }

    #[derive(Debug, Error)]
    pub enum RequestError {
        #[error("Send failed: {0}")]
        SendFailed(#[from] SendError),
    }

    #[derive(Debug, Error)]
    pub enum DeferredActionError<T> {
        #[error("Background runner is already closed!")]
        BackgroundRunnerClosed,

        #[error("Channel is at capacity!")]
        ChannelAtCapacity(T),
    }

    #[derive(Debug, Error)]
    pub enum ReceiveResponseError {
        #[error("RPC server was closed.")]
        ServerClosed,

        #[error("RPC client was closed.")]
        ClientClosed,
    }

    // ==== DeferredActionError ====

    pub(crate) fn convert_deferred_write_err(
        e: TrySendError<DeferredDirective>,
    ) -> DeferredActionError<Bytes> {
        match e {
            TrySendError::Closed(_) => DeferredActionError::BackgroundRunnerClosed,
            TrySendError::Full(DeferredDirective::WriteNoti(x)) => {
                DeferredActionError::ChannelAtCapacity(x)
            }
            TrySendError::Full(_) => unreachable!(),
        }
    }

    pub(crate) fn convert_deferred_action_err(
        e: TrySendError<DeferredDirective>,
    ) -> DeferredActionError<()> {
        match e {
            TrySendError::Closed(_) => DeferredActionError::BackgroundRunnerClosed,
            TrySendError::Full(_) => DeferredActionError::ChannelAtCapacity(()),
        }
    }
}

use bytes::BytesMut;
use tokio::sync::mpsc;

use crate::codec::Codec;
use crate::defs::RequestId;
use crate::io::AsyncFrameWrite;

use self::error::*;

mod req_rep {
    use std::{
        borrow::Cow,
        future::Future,
        mem::replace,
        sync::{atomic::AtomicU32, Arc, Weak},
        task::{Poll, Waker},
    };

    use bytes::Bytes;
    use hashbrown::HashMap;
    use parking_lot::{Mutex, RwLock};
    use serde::de::Deserialize;

    use crate::{codec::Codec, defs::RequestId};

    use super::{error::ReceiveResponseError, ReceiveResponse};

    /// Response message from RPC server.
    pub struct Response(Arc<dyn Codec>, Bytes);

    /// A context for pending RPC requests.
    #[derive(Debug)]
    pub(super) struct RequestContext {
        /// Codec of owning RPC connection.
        codec: Weak<dyn Codec>,

        /// Request ID generator. Rotates every 2^32 requests.
        ///
        /// It naively expects that the request ID is not reused until 2^32 requests are made.
        req_id_gen: AtomicU32,

        /// A set of pending requests that are waiting to be responded.        
        pending_tasks: RwLock<HashMap<RequestId, Mutex<PendingTask>>>,
    }

    #[derive(Debug)]
    struct PendingTask {
        registered_waker: Option<Waker>,
        response: ResponseData,
    }

    #[derive(Debug)]
    enum ResponseData {
        NotReady,
        Ready(Bytes),
        Closed,
        Unreachable,
    }

    // ========================================================== ReceiveResponse ===|

    #[derive(Default)]
    pub(super) enum ReceiveResponseState {
        /// A waker that is registered to the pending task.
        #[default]
        Init,
        Pending(Waker),

        /// Required to implement [`ReceiveResponse::into_owned`]
        Expired,
    }

    impl<'a> Future for ReceiveResponse<'a> {
        type Output = Result<Response, ReceiveResponseError>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            let this = self.get_mut();

            /*
                TODO: Implement state machine for response receiving.

                - Init: Fetch reference to the slot. Try to retrieve the response from the slot.
                - Pending: Try to retrieve response ...
                - Expired: panic!
            */

            let lc_entry = this.reqs.pending_tasks.read();
            let mut lc_slot = lc_entry
                .get(&this.req_id)
                .expect("'req_id' not found; seems polled after ready!")
                .lock();

            // Check if we're ready to retrieve response
            match &mut lc_slot.response {
                ResponseData::NotReady => (),
                ResponseData::Unreachable => unreachable!("Polled after ready"),
                ResponseData::Closed => {
                    return Poll::Ready(Err(ReceiveResponseError::ServerClosed))
                }
                resp @ ResponseData::Ready(_) => {
                    let ResponseData::Ready(buf) = replace(resp, ResponseData::Unreachable) else {
                        unreachable!()
                    };

                    // Drops lock before promoting codec ptr ... (very slight performance gain)
                    drop(lc_slot);
                    drop(lc_entry);

                    let codec = this
                        .reqs
                        .codec
                        .upgrade()
                        .ok_or(ReceiveResponseError::ClientClosed)?;

                    return Poll::Ready(Ok(Response(codec, buf)));
                }
            }

            let new_waker = match &mut this.state {
                ReceiveResponseState::Init => {
                    let waker = cx.waker().clone();
                    this.state = ReceiveResponseState::Pending(waker.clone());
                    Some(waker)
                }
                ReceiveResponseState::Pending(waker) => {
                    let new_waker = cx.waker();
                    if waker.will_wake(new_waker) {
                        None // We can reuse the waker (optimize it away)
                    } else {
                        *waker = new_waker.clone();
                        Some(waker.clone())
                    }
                }
                ReceiveResponseState::Expired => panic!("Polled after ready"),
            };

            if let Some(wk) = new_waker {
                lc_slot.registered_waker = Some(wk);
            }

            Poll::Pending
        }
    }

    impl<'a> Drop for ReceiveResponse<'a> {
        fn drop(&mut self) {
            if matches!(self.state, ReceiveResponseState::Expired) {
                return;
            }

            let _e = self.reqs.pending_tasks.write().remove(&self.req_id);
            debug_assert!(
                _e.is_some(),
                "ReceiveResponse is dropped before polled to ready!"
            );
        }
    }

    // ======== ReceiveResponse ======== //

    impl<'a> ReceiveResponse<'a> {
        /// Elevate the lifetime of the response to `'static`.
        pub fn into_owned(mut self) -> ReceiveResponse<'static> {
            ReceiveResponse {
                reqs: Cow::Owned((*self.reqs).clone()),
                req_id: self.req_id,
                state: replace(&mut self.state, ReceiveResponseState::Expired),
            }
        }

        pub fn request_id(&self) -> RequestId {
            self.req_id
        }
    }

    // ========================================================== Response ===|

    impl Response {
        pub fn parse<R>(&self) -> erased_serde::Result<R>
        where
            R: for<'de> Deserialize<'de>,
        {
            todo!()
        }
    }

    // ==== RequestContext ====

    impl RequestContext {
        pub(super) fn allocate_new_request(&self) -> RequestId {
            todo!()
        }

        /// Mark the request ID as free.
        ///
        /// # Panics
        ///
        /// Panics if the request ID is not found from the registry
        pub(super) fn free_id(&self, id: RequestId) {
            todo!()
        }

        /// Mark the request ID as aborted; which won't be reused until rotation.
        pub(super) fn abort_id(&self, id: RequestId) {
            todo!()
        }

        /// Set the response for the request ID.
        pub(crate) fn set_response(&self, id: RequestId, resp: Response) {
            todo!()
        }
    }
}

pub use req_rep::Response;
use req_rep::*;

pub mod builder {
    //! # Builder for RPC connection

    ///
    pub struct Builder<Wr, Rd, U, C> {
        _0: std::marker::PhantomData<(Wr, Rd, U, C)>,
    }
}

mod driver {
    use bytes::Bytes;

    use crate::defs::RequestId;

    /// A message to be sent to the background dedicated writer task.
    pub(crate) enum DeferredDirective {
        /// Close the writer transport immediately after receiving this message.
        CloseImmediately,

        /// Close the writer transport after flushing all pending write requests.
        ///
        /// The rx channel, which is used to receive this message, will be closed right after this
        /// message is received.
        CloseAfterFlush,

        /// Flush the writer transport.
        Flush,

        /// Write a notification message.
        WriteNoti(Bytes),

        /// Write a request message. If the sending of the request is aborted by the writer, the
        /// request message will be revoked and will wake up the pending task.
        WriteReq(Bytes, RequestId),
    }
}

use driver::*;

// ========================================================== RpcContext ===|

impl<U> dyn RpcContext<U> {
    pub(crate) async fn write_frame(&self, buf: Bytes) -> std::io::Result<()> {
        todo!()
    }
}

// ========================================================== NotifyClient ===|

/// Implements notification methods for [`NotifyClient`].
impl<U: RpcUserData> NotifyClient<U> {
    /// Send a RPC notification.
    pub async fn notify<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buf.clear();
        self.inner.codec().encode_notify(method, params, buf)?;
        self.inner.write_frame(buf.split().freeze()).await?;
        Ok(())
    }

    pub fn notify_deferred<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        buf.clear();
        self.inner.codec().encode_notify(method, params, buf)?;
        self.write_frame_deferred(DeferredDirective::WriteNoti(buf.split().freeze()))?;

        Ok(())
    }

    fn write_frame_deferred(
        &self,
        buf: DeferredDirective,
    ) -> Result<(), DeferredActionError<Bytes>> {
        self.tx_deferred
            .try_send(buf)
            .map_err(error::convert_deferred_write_err)
    }

    pub fn user_data(&self) -> &U {
        self.inner.user_data()
    }

    pub fn codec(&self) -> &dyn Codec {
        self.inner.codec()
    }

    pub fn cloned_codec(&self) -> Arc<dyn Codec> {
        self.inner.self_as_codec()
    }

    /// Que a close request to the background writer task. It will first flush all remaining data
    /// transfer request, then will close the writer. If background channel is already closed,
    /// returns `Err`.
    ///
    /// If `drop_after_this` is specified, any deferred outbound message will be dropped.
    pub fn close_writer(&self, drop_after_this: bool) -> Result<(), DeferredActionError<()>> {
        self.tx_deferred
            .try_send(if drop_after_this {
                DeferredDirective::CloseImmediately
            } else {
                DeferredDirective::CloseAfterFlush
            })
            .map_err(error::convert_deferred_action_err)
    }

    /// Closes the writer channel immediately. This will invalidate all pending write requests.
    pub async fn force_close_writer(self) -> std::io::Result<()> {
        let mut writer = self.inner.writer().lock().await;

        // SAFETY: `writer` memory goes nowhere during locked.
        poll_fn(|cx| unsafe { Pin::new_unchecked(&mut *writer).poll_close(cx) }).await
    }

    pub async fn flush_writer(&self) -> std::io::Result<()> {
        let mut writer = self.inner.writer().lock().await;

        // SAFETY: `writer` memory goes nowhere during locked.
        poll_fn(|cx| unsafe { Pin::new_unchecked(&mut *writer).poll_flush(cx) }).await
    }

    /// Requests flush to the background writer task. As actual flush operation is done in
    /// background writer task, you can't get the actual result of the flush operation.
    pub fn flush_writer_deferred(&self) -> Result<(), DeferredActionError<()>> {
        self.tx_deferred
            .try_send(DeferredDirective::Flush)
            .map_err(error::convert_deferred_action_err)
    }
}

impl<U> Clone for NotifyClient<U> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            tx_deferred: self.tx_deferred.clone(),
        }
    }
}

// ========================================================== Client ===|

/// Implements request methods for [`Client`].
impl<U: RpcUserData> Client<U> {
    pub async fn request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, RequestError> {
        let resp = self.encode_request(buf, method, params)?;

        self.inner
            .inner
            .write_frame(buf.split().freeze())
            .await
            .map_err(SendError::from)?;

        Ok(resp)
    }

    pub fn request_deferred<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, RequestError> {
        let resp = self.encode_request(buf, method, params)?;
        let request_id = resp.request_id();

        self.write_frame_deferred(DeferredDirective::WriteReq(
            buf.split().freeze(),
            request_id,
        ))
        .map_err(SendError::from)?;

        Ok(resp)
    }

    fn encode_request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, SendError> {
        buf.clear();

        let request_id = self.req.allocate_new_request();
        self.codec()
            .encode_request(request_id, method, params, buf)
            .map_err(|e| {
                self.req.free_id(request_id);
                SendError::from(e)
            })?;

        Ok(ReceiveResponse {
            reqs: Cow::Borrowed(&self.req),
            req_id: request_id,
            state: Default::default(),
        })
    }
}

impl<U> Clone for Client<U> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            req: self.req.clone(),
        }
    }
}

/// Provides handy way to access [`NotifyClient`] methods in [`Client`].
impl<U> std::ops::Deref for Client<U> {
    type Target = NotifyClient<U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
