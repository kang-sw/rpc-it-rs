use std::borrow::Cow;

use std::sync::Arc;
use std::sync::Weak;

use bytes::BytesMut;
use tokio::sync::mpsc;

use crate::codec::error::EncodeError;
use crate::codec::Codec;
use crate::defs::RequestId;

use self::error::*;
use self::req_rep::*;

pub use self::builder::create_builder;
pub use self::driver::*;
pub use self::req_rep::Response;

// ==== Basic RPC ====

/// Generic trait for underlying RPC connection.
pub(crate) trait RpcContext<U: UserData>: std::fmt::Debug {
    fn self_as_codec(&self) -> Arc<dyn Codec>;
    fn codec(&self) -> &dyn Codec;
    fn user_data(&self) -> &U;
    fn shutdown_rx_channel(&self);
}

/// A trait constraint for user data type of a RPC connection.
pub trait UserData: Send + Sync + 'static {}
impl<T> UserData for T where T: Send + Sync + 'static {}

/// A RPC client handle which can only send RPC notifications.
#[derive(Debug)]
pub struct NotifySender<U> {
    context: Arc<dyn RpcContext<U>>,
    tx_deferred: mpsc::Sender<DeferredDirective>,
}

/// A weak RPC client handle which can only send RPC notifications.
///
/// Should be upgraded to [`NotifySender`] before use.
#[derive(Debug)]
pub struct WeakNotifySender<U> {
    context: Weak<dyn RpcContext<U>>,
    tx_deferred: mpsc::WeakSender<DeferredDirective>,
}

// ==== Request Capability ====

/// A RPC client handle which can send RPC requests and notifications.
///
/// It is super-set of [`NotifySender`].
#[derive(Debug)]
pub struct RequestSender<U> {
    inner: NotifySender<U>,
    req: Arc<RequestContext>,
}

/// A weak RPC client handle which can send RPC requests and notifications.
///
/// Should be upgraded to [`RequestSender`] before use.
#[derive(Debug)]
pub struct WeakRequestSender<U> {
    inner: WeakNotifySender<U>,
    req: Weak<RequestContext>,
}

/// An awaitable response for a sent RPC request
pub struct ReceiveResponse<'a> {
    reqs: Cow<'a, Arc<RequestContext>>,
    req_id: RequestId,
    state: req_rep::ReceiveResponseState,
}

/// A receiver which deals with inbound notifies / requests.
pub struct Receiver {
    context: Arc<dyn RpcContext<()>>,
    rx: mpsc::Receiver<InboundMessageInner>,
}

// ========================================================== Details ===|

pub mod builder;
mod driver;
pub mod error;
mod req_rep;

// ========================================================== NotifySender ===|

/// Implements notification methods for [`NotifySender`].
impl<U: UserData> NotifySender<U> {
    pub async fn notify<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendMsgError> {
        buf.clear();
        self.context.codec().encode_notify(method, params, buf)?;
        self.send_frame(DeferredDirective::WriteNoti(buf.split().freeze()))
            .await?;

        Ok(())
    }

    pub fn try_notify<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), TrySendMsgError> {
        buf.clear();
        self.context.codec().encode_notify(method, params, buf)?;
        self.try_send_frame(DeferredDirective::WriteNoti(buf.split().freeze()))?;

        Ok(())
    }

    fn try_send_frame(&self, buf: DeferredDirective) -> Result<(), TrySendMsgError> {
        self.tx_deferred
            .try_send(buf)
            .map_err(error::convert_deferred_write_err)
    }

    async fn send_frame(&self, buf: DeferredDirective) -> Result<(), SendMsgError> {
        self.tx_deferred
            .send(buf)
            .await
            .map_err(|_| SendMsgError::BackgroundRunnerClosed)
    }

    pub fn user_data(&self) -> &U {
        self.context.user_data()
    }

    pub fn codec(&self) -> &dyn Codec {
        self.context.codec()
    }

    pub fn cloned_codec(&self) -> Arc<dyn Codec> {
        self.context.self_as_codec()
    }

    /// Que a close request to the background writer task. It will first flush all remaining data
    /// transfer request, then will close the writer. If background channel is already closed,
    /// returns `Err`.
    ///
    /// If `drop_after_this` is specified, any deferred outbound message will be dropped.
    pub fn try_shutdown_writer(&self, drop_after_this: bool) -> Result<(), TrySendMsgError> {
        self.tx_deferred
            .try_send(if drop_after_this {
                DeferredDirective::CloseImmediately
            } else {
                DeferredDirective::CloseAfterFlush
            })
            .map_err(error::convert_deferred_action_err)
    }

    /// See [`NotifySender::try_close_writer`]
    pub async fn shutdown_writer(&self, drop_after_this: bool) -> Result<(), TrySendMsgError> {
        self.tx_deferred
            .send(if drop_after_this {
                DeferredDirective::CloseImmediately
            } else {
                DeferredDirective::CloseAfterFlush
            })
            .await
            .map_err(|_| TrySendMsgError::BackgroundRunnerClosed)
    }

    /// Shutdown the background reader task. This will close the reader channel, and will wake up
    /// all pending tasks, delivering [`ReceiveResponseError::Shutdown`] error.
    pub fn shutdown_reader(&self) {
        self.context.shutdown_rx_channel();
    }

    /// Requests flush to the background writer task. As actual flush operation is done in
    /// background writer task, you can't get the actual result of the flush operation.
    pub fn try_flush_writer(&self) -> Result<(), TrySendMsgError> {
        self.tx_deferred
            .try_send(DeferredDirective::Flush)
            .map_err(error::convert_deferred_action_err)
    }

    /// See [`NotifySender::try_flush_writer`]
    pub async fn flush_writer(&self) -> Result<(), TrySendMsgError> {
        self.tx_deferred
            .send(DeferredDirective::Flush)
            .await
            .map_err(|_| TrySendMsgError::BackgroundRunnerClosed)
    }

    /// Downgrade this handle to a weak handle.
    pub fn downgrade(&self) -> WeakNotifySender<U> {
        WeakNotifySender {
            context: Arc::downgrade(&self.context),
            tx_deferred: self.tx_deferred.downgrade(),
        }
    }
}

impl<U> Clone for NotifySender<U> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            tx_deferred: self.tx_deferred.clone(),
        }
    }
}

impl<U> WeakNotifySender<U> {
    /// Upgrade this handle to a strong handle.
    pub fn upgrade(&self) -> Option<NotifySender<U>> {
        self.context
            .upgrade()
            .zip(self.tx_deferred.upgrade())
            .map(|(context, tx_deferred)| NotifySender {
                context,
                tx_deferred,
            })
    }
}

impl<U> Clone for WeakNotifySender<U> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            tx_deferred: self.tx_deferred.clone(),
        }
    }
}

// ========================================================== RequestSender ===|

/// Implements request methods for [`RequestSender`].
impl<U: UserData> RequestSender<U> {
    pub async fn request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, SendMsgError> {
        let resp = self.encode_request(buf, method, params)?;
        let request_id = resp.request_id();

        self.send_frame(DeferredDirective::WriteReq(
            buf.split().freeze(),
            request_id,
        ))
        .await?;

        Ok(resp)
    }

    pub fn try_request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, TrySendMsgError> {
        let resp = self.encode_request(buf, method, params)?;
        let request_id = resp.request_id();

        self.try_send_frame(DeferredDirective::WriteReq(
            buf.split().freeze(),
            request_id,
        ))?;

        Ok(resp)
    }

    fn encode_request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse, EncodeError> {
        buf.clear();

        let request_id = self.req.allocate_new_request();
        let encode_result = self.codec().encode_request(request_id, method, params, buf);

        if let Err(err) = encode_result {
            self.req.cancel_request_alloc(request_id);
            return Err(err);
        }

        Ok(ReceiveResponse {
            reqs: Cow::Borrowed(&self.req),
            req_id: request_id,
            state: Default::default(),
        })
    }

    pub fn downgrade(&self) -> WeakRequestSender<U> {
        WeakRequestSender {
            inner: self.inner.downgrade(),
            req: Arc::downgrade(&self.req),
        }
    }
}

impl<U> Clone for RequestSender<U> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            req: self.req.clone(),
        }
    }
}

/// Provides handy way to access [`NotifySender`] methods in [`RequestSender`].
impl<U> std::ops::Deref for RequestSender<U> {
    type Target = NotifySender<U>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<U> WeakRequestSender<U> {
    pub fn upgrade(&self) -> Option<RequestSender<U>> {
        self.inner
            .upgrade()
            .zip(self.req.upgrade())
            .map(|(inner, req)| RequestSender { inner, req })
    }
}

impl<U> Clone for WeakRequestSender<U> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            req: self.req.clone(),
        }
    }
}
