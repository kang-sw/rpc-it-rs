use std::{marker::PhantomData, num::NonZeroUsize};

use bytes::Bytes;
use erased_serde::Serialize;

use super::*;

/// A RPC client handle which can only send RPC notifications.
#[repr(transparent)]
pub struct NotifySender<U, C> {
    pub(super) context: Arc<RpcCore<U, C>>,
}

/// A weak RPC client handle which can only send RPC notifications.
///
/// Should be upgraded to [`NotifySender`] before use.
#[derive(Debug)]
pub struct WeakNotifySender<U, C> {
    pub(super) context: Weak<RpcCore<U, C>>,
}

// ==== Request Capability ====

/// A RPC client handle which can send RPC requests and notifications.
///
/// It is super-set of [`NotifySender`].
pub struct RequestSender<U, C> {
    pub(super) inner: NotifySender<U, C>,
}

/// A weak RPC client handle which can send RPC requests and notifications.
///
/// Should be upgraded to [`RequestSender`] before use.
#[derive(Debug)]
pub struct WeakRequestSender<U, C> {
    inner: WeakNotifySender<U, C>,
}

/// An awaitable response for a sent RPC request
pub struct ReceiveResponse<'a, U, C> {
    pub(super) owner: Cow<'a, RequestSender<U, C>>,
    pub(super) req_id: RequestId,
    pub(super) state: req_rep::ReceiveResponseState,
}

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

    /// Write a notification/response message.
    WriteMsg(Bytes),

    /// Write burst of notification messages.
    WriteMsgBurst(Vec<Bytes>),

    /// Write a request message. If the sending of the request is aborted by the writer, the
    /// request message will be revoked and will wake up the pending task.
    WriteReqMsg(Bytes, RequestId),
}

/// A message that was encoded but not yet sent to client.
#[derive(Debug)]
pub struct PreparedNoti<C> {
    _c: PhantomData<C>,
    data: Bytes,
    hash: Option<NonZeroUsize>,
}

// ==== Debug Trait Impls ====

/// Implements notification methods for [`NotifySender`].
impl<U: UserData, C: Codec> std::fmt::Debug for NotifySender<U, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifySender")
            .field("context", &self.context)
            .finish()
    }
}

/// Implements notification methods for [`NotifySender`].
impl<U: UserData, C: Codec> std::fmt::Debug for RequestSender<U, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifySender")
            .field("context", &self.context)
            .finish()
    }
}

// ========================================================== NotifySender ===|

impl<U: UserData, C: Codec> NotifySender<U, C> {
    pub async fn notify<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendMsgError> {
        buf.clear();
        self.context.codec().encode_notify(method, params, buf)?;
        self.send_frame(DeferredDirective::WriteMsg(buf.split().freeze()))
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
        self.try_send_frame(DeferredDirective::WriteMsg(buf.split().freeze()))?;

        Ok(())
    }

    fn tx_deferred(&self) -> &mpsc::Sender<DeferredDirective> {
        // SAFETY: Once [`NotifySender`] is created, the writer channel is always defined.
        unsafe { self.context.tx_deferred().unwrap_unchecked() }
    }

    fn try_send_frame(&self, buf: DeferredDirective) -> Result<(), TrySendMsgError> {
        self.tx_deferred()
            .try_send(buf)
            .map_err(error::convert_deferred_write_err)
    }

    async fn send_frame(&self, buf: DeferredDirective) -> Result<(), SendMsgError> {
        self.tx_deferred()
            .send(buf)
            .await
            .map_err(|_| SendMsgError::ChannelClosed)
    }

    pub fn user_data(&self) -> &U {
        self.context.user_data()
    }

    pub fn codec(&self) -> &C {
        self.context.codec()
    }

    /// Upgrade this handle to request handle. Fails if request feature was not enabled at first.
    pub fn try_into_request_sender(self) -> Result<RequestSender<U, C>, Self> {
        if self.context.request_context().is_some() {
            Ok(RequestSender { inner: self })
        } else {
            Err(self)
        }
    }

    /// Que a close request to the background writer task. It will first flush all remaining data
    /// transfer request, then will close the writer. If background channel is already closed,
    /// returns `Err`.
    ///
    /// If `drop_after_this` is specified, any deferred outbound message will be dropped.
    pub fn try_shutdown_writer(&self, drop_after_this: bool) -> Result<(), TrySendMsgError> {
        let tx_deferred = self.tx_deferred();

        tx_deferred
            .try_send(if drop_after_this {
                DeferredDirective::CloseImmediately
            } else {
                DeferredDirective::CloseAfterFlush
            })
            .map_err(error::convert_deferred_action_err)?;

        // Then prevent further messages from being sent immediately.
        tx_deferred.close();

        Ok(())
    }

    /// See [`NotifySender::try_close_writer`]
    pub async fn shutdown_writer(&self, discard_unsent: bool) -> Result<(), TrySendMsgError> {
        let tx_deferred = self.tx_deferred();

        tx_deferred
            .send(if discard_unsent {
                DeferredDirective::CloseImmediately
            } else {
                DeferredDirective::CloseAfterFlush
            })
            .await
            .map_err(|_| TrySendMsgError::ChannelClosed)?;

        // Then prevent further messages from being sent immediately.
        tx_deferred.close();

        Ok(())
    }

    /// Requests flush to the background writer task. As actual flush operation is done in
    /// background writer task, you can't get the actual result of the flush operation.
    pub fn try_flush_writer(&self) -> Result<(), TrySendMsgError> {
        self.tx_deferred()
            .try_send(DeferredDirective::Flush)
            .map_err(error::convert_deferred_action_err)
    }

    /// See [`NotifySender::try_flush_writer`]
    pub async fn flush_writer(&self) -> Result<(), TrySendMsgError> {
        self.tx_deferred()
            .send(DeferredDirective::Flush)
            .await
            .map_err(|_| TrySendMsgError::ChannelClosed)
    }

    /// Downgrade this handle to a weak handle.
    pub fn downgrade(&self) -> WeakNotifySender<U, C> {
        WeakNotifySender {
            context: Arc::downgrade(&self.context),
        }
    }
}

impl<U, C> Clone for NotifySender<U, C> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
        }
    }
}

impl<U, C> WeakNotifySender<U, C> {
    /// Upgrade this handle to a strong handle.
    pub fn upgrade(&self) -> Option<NotifySender<U, C>> {
        self.context
            .upgrade()
            .map(|context| NotifySender { context })
    }
}

impl<U, C> Clone for WeakNotifySender<U, C> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
        }
    }
}

impl<C> Clone for PreparedNoti<C> {
    fn clone(&self) -> Self {
        Self {
            _c: PhantomData,
            hash: self.hash,
            data: self.data.clone(),
        }
    }
}

// ========================================================== RequestSender ===|

/// Implements request methods for [`RequestSender`].
impl<U: UserData, C: Codec> RequestSender<U, C> {
    pub async fn request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse<U, C>, SendMsgError> {
        let resp = self
            .encode_request(buf, method, params)
            .ok_or(SendMsgError::ReceiverExpired)??;
        let request_id = resp.request_id();

        self.send_frame(DeferredDirective::WriteReqMsg(
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
    ) -> Result<ReceiveResponse<U, C>, TrySendMsgError> {
        let resp = self
            .encode_request(buf, method, params)
            .ok_or(TrySendMsgError::ReceiverExpired)??;
        let request_id = resp.request_id();

        self.try_send_frame(DeferredDirective::WriteReqMsg(
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
    ) -> Option<Result<ReceiveResponse<U, C>, EncodeError>> {
        buf.clear();

        let reqs = self.reqs();
        let request_id = reqs.allocate_new_request()?;
        let encode_result = self.codec().encode_request(request_id, method, params, buf);

        if let Err(err) = encode_result {
            self.reqs().cancel_request_alloc(request_id);
            return Some(Err(err));
        }

        Some(Ok(ReceiveResponse {
            owner: Cow::Borrowed(self),
            req_id: request_id,
            state: Default::default(),
        }))
    }

    pub fn downgrade(&self) -> WeakRequestSender<U, C> {
        WeakRequestSender {
            inner: self.inner.downgrade(),
        }
    }
}

impl<U, C> RequestSender<U, C> {
    /// Unwraps request context from this handle; This is valid since it's unconditionally defined
    /// when [`RequestSender`] is created.
    pub(super) fn reqs(&self) -> &RequestContext<C> {
        self.context.request_context().unwrap()
    }
}

impl<U, C> Clone for RequestSender<U, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Provides handy way to access [`NotifySender`] methods in [`RequestSender`].
impl<U, C> std::ops::Deref for RequestSender<U, C> {
    type Target = NotifySender<U, C>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<U, C> WeakRequestSender<U, C> {
    pub fn upgrade(&self) -> Option<RequestSender<U, C>> {
        self.inner.upgrade().map(|inner| RequestSender { inner })
    }
}

impl<U, C> Clone for WeakRequestSender<U, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ========================================================== Broadcast ===|

pub enum NotiBurstOps<C> {
    Mono(PreparedNoti<C>),
    Burst(Vec<PreparedNoti<C>>),
}

impl<U, C> NotifySender<U, C>
where
    U: UserData,
    C: Codec,
{
    /// Prepare pre-encoded notification message.
    pub fn prepare<S: Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &S,
    ) -> Result<PreparedNoti<C>, EncodeError> {
        todo!()
    }

    ///
    pub fn burst(&self, burst: impl Into<NotiBurstOps<C>>) -> Result<NotiBurstOps<C>, EncodeError> {
        todo!()
    }
}
