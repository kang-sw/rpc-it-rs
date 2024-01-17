use std::{marker::PhantomData, num::NonZeroUsize};

use bytes::Bytes;

use super::*;

/// A RPC client handle which can only send RPC notifications.
#[repr(transparent)]
pub struct NotifySender<R: Config> {
    pub(super) context: Arc<RpcCore<R>>,
}

/// A weak RPC client handle which can only send RPC notifications.
///
/// Should be upgraded to [`NotifySender`] before use.
#[derive(Debug)]
pub struct WeakNotifySender<R: Config> {
    pub(super) context: Weak<RpcCore<R>>,
}

// ==== Request Capability ====

/// A RPC client handle which can send RPC requests and notifications.
///
/// It is super-set of [`NotifySender`].
pub struct RequestSender<R: Config> {
    pub(super) inner: NotifySender<R>,
}

/// A weak RPC client handle which can send RPC requests and notifications.
///
/// Should be upgraded to [`RequestSender`] before use.
#[derive(Debug)]
pub struct WeakRequestSender<R: Config> {
    inner: WeakNotifySender<R>,
}

/// An awaitable response for a sent RPC request
pub struct ReceiveResponse<'a, R: Config> {
    pub(super) owner: Cow<'a, RequestSender<R>>,
    pub(super) req_id: RequestId,
    pub(super) state: req_rep::ReceiveResponseState,
}

/// A message to be sent to the background dedicated writer task.
pub(crate) enum WriterDirective {
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

// ==== Debug Trait Impls ====

/// Implements notification methods for [`NotifySender`].
impl<R: Config> std::fmt::Debug for NotifySender<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifySender")
            .field("context", &self.context)
            .finish()
    }
}

/// Implements notification methods for [`NotifySender`].
impl<R: Config> std::fmt::Debug for RequestSender<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifySender")
            .field("context", &self.context)
            .finish()
    }
}

// ========================================================== NotifySender ===|

impl<R: Config> NotifySender<R> {
    pub async fn notify<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<(), SendMsgError> {
        buf.clear();
        self.context.codec.encode_notify(method, params, buf)?;
        self.send_frame(WriterDirective::WriteMsg(buf.split().freeze()))
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
        self.context.codec.encode_notify(method, params, buf)?;
        self.try_send_frame(WriterDirective::WriteMsg(buf.split().freeze()))?;

        Ok(())
    }

    fn tx_deferred(&self) -> &mpsc::Sender<WriterDirective> {
        // SAFETY: Once [`NotifySender`] is created, the writer channel is always defined.
        unsafe { self.context.tx_deferred().unwrap_unchecked() }
    }

    fn try_send_frame(&self, buf: WriterDirective) -> Result<(), TrySendMsgError> {
        self.tx_deferred()
            .try_send(buf)
            .map_err(error::convert_deferred_write_err)
    }

    async fn send_frame(&self, buf: WriterDirective) -> Result<(), SendMsgError> {
        self.tx_deferred()
            .send(buf)
            .await
            .map_err(|_| SendMsgError::ChannelClosed)
    }

    pub fn user_data(&self) -> &R::UserData {
        self.context.user_data()
    }

    pub fn codec(&self) -> &R::Codec {
        &self.context.codec
    }

    /// Upgrade this handle to request handle. Fails if request feature was not enabled at first.
    pub fn try_into_request_sender(self) -> Result<RequestSender<R>, Self> {
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
                WriterDirective::CloseImmediately
            } else {
                WriterDirective::CloseAfterFlush
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
                WriterDirective::CloseImmediately
            } else {
                WriterDirective::CloseAfterFlush
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
            .try_send(WriterDirective::Flush)
            .map_err(error::convert_deferred_action_err)
    }

    /// See [`NotifySender::try_flush_writer`]
    pub async fn flush_writer(&self) -> Result<(), TrySendMsgError> {
        self.tx_deferred()
            .send(WriterDirective::Flush)
            .await
            .map_err(|_| TrySendMsgError::ChannelClosed)
    }

    /// Downgrade this handle to a weak handle.
    pub fn downgrade(&self) -> WeakNotifySender<R> {
        WeakNotifySender {
            context: Arc::downgrade(&self.context),
        }
    }
}

impl<R: Config> Clone for NotifySender<R> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
        }
    }
}

impl<R: Config> WeakNotifySender<R> {
    /// Upgrade this handle to a strong handle.
    pub fn upgrade(&self) -> Option<NotifySender<R>> {
        self.context
            .upgrade()
            .map(|context| NotifySender { context })
    }
}

impl<R: Config> Clone for WeakNotifySender<R> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
        }
    }
}

// ========================================================== RequestSender ===|

/// Implements request methods for [`RequestSender`].
impl<R: Config> RequestSender<R> {
    pub async fn request<T: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &T,
    ) -> Result<ReceiveResponse<R>, SendMsgError> {
        let resp = self
            .encode_request(buf, method, params)
            .ok_or(SendMsgError::ReceiverExpired)??;
        let request_id = resp.request_id();

        self.send_frame(WriterDirective::WriteReqMsg(
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
    ) -> Result<ReceiveResponse<R>, TrySendMsgError> {
        let resp = self
            .encode_request(buf, method, params)
            .ok_or(TrySendMsgError::ReceiverExpired)??;
        let request_id = resp.request_id();

        self.try_send_frame(WriterDirective::WriteReqMsg(
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
    ) -> Option<Result<ReceiveResponse<R>, EncodeError>> {
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

    pub fn downgrade(&self) -> WeakRequestSender<R> {
        WeakRequestSender {
            inner: self.inner.downgrade(),
        }
    }
}

impl<R: Config> RequestSender<R> {
    /// Unwraps request context from this handle; This is valid since it's unconditionally defined
    /// when [`RequestSender`] is created.
    pub(super) fn reqs(&self) -> &RequestContext<R::Codec> {
        self.context.request_context().unwrap()
    }
}

impl<R: Config> Clone for RequestSender<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Provides handy way to access [`NotifySender`] methods in [`RequestSender`].
impl<R: Config> std::ops::Deref for RequestSender<R> {
    type Target = NotifySender<R>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<R: Config> WeakRequestSender<R> {
    pub fn upgrade(&self) -> Option<RequestSender<R>> {
        self.inner.upgrade().map(|inner| RequestSender { inner })
    }
}

impl<R: Config> Clone for WeakRequestSender<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ========================================================== Broadcast ===|

/// A message that was encoded but not yet sent to client.
#[derive(Debug)]
pub struct PreparedPacket<C> {
    _c: PhantomData<C>,
    data: Bytes,
    hash: NonZeroUsize,
}

pub enum PacketWriteBurst<C> {
    ErrHashMismatch,
    Empty,
    Mono(PreparedPacket<C>),
    Burst(NonZeroUsize, Vec<Bytes>),
}

// ==== PreparedPacket ====

impl<C> PreparedPacket<C>
where
    C: Codec,
{
    pub(super) fn new_unchecked(data: Bytes, hash: NonZeroUsize) -> Self {
        Self {
            _c: PhantomData,
            data,
            hash,
        }
    }
}

impl<C> Clone for PreparedPacket<C> {
    fn clone(&self) -> Self {
        Self {
            _c: PhantomData,
            hash: self.hash,
            data: self.data.clone(),
        }
    }
}

// ==== Burst API ====

impl<R> NotifySender<R>
where
    R: Config,
{
    /// Prepare pre-encoded notification message. Request can't be prepared as there's no concept of
    /// broadcast or reuse in request.
    pub fn encode_notify<S: serde::Serialize>(
        &self,
        buf: &mut BytesMut,
        method: &str,
        params: &S,
    ) -> Result<PreparedPacket<R::Codec>, EncodeError> {
        buf.clear();

        let hash: NonZeroUsize = (self.context.codec.codec_type_hash_ptr() as usize)
            .try_into()
            .map_err(|_| EncodeError::NotReusable)?;

        self.context.codec.encode_notify(method, params, buf)?;

        Ok(PreparedPacket {
            _c: PhantomData,
            data: buf.split().freeze(),
            hash,
        })
    }

    ///
    pub async fn burst(
        &self,
        burst: impl Into<PacketWriteBurst<R::Codec>>,
    ) -> Result<usize, SendMsgError> {
        let Some((count, msg)) = self.burst_into_directive(burst.into())? else {
            return Ok(0);
        };

        self.send_frame(msg).await.map(|_| count)
    }

    pub fn try_burst(
        &self,
        burst: impl Into<PacketWriteBurst<R::Codec>>,
    ) -> Result<usize, TrySendMsgError> {
        let Some((count, msg)) = self.burst_into_directive(burst.into())? else {
            return Ok(0);
        };

        self.try_send_frame(msg).map(|_| count)
    }

    /// Returns
    /// - `Ok(None)` if the burst is empty
    /// - `Ok(Some(_))` if the burst can be sent
    /// - `Err(())` if burst codec hash mismatches
    fn burst_into_directive(
        &self,
        burst: PacketWriteBurst<R::Codec>,
    ) -> Result<Option<(usize, WriterDirective)>, EncodeError> {
        let codec_hash = self.context.codec.codec_type_hash_ptr() as usize;

        match burst {
            PacketWriteBurst::Empty => Ok(None),
            PacketWriteBurst::ErrHashMismatch => Err(EncodeError::NotReusable),
            PacketWriteBurst::Mono(pkt) => {
                if pkt.hash.get() == codec_hash {
                    Ok(Some((1, WriterDirective::WriteMsg(pkt.data))))
                } else {
                    Err(EncodeError::NotReusable)
                }
            }
            PacketWriteBurst::Burst(hsah, chunks) => {
                if hsah.get() == codec_hash {
                    Ok(Some((chunks.len(), WriterDirective::WriteMsgBurst(chunks))))
                } else {
                    Err(EncodeError::NotReusable)
                }
            }
        }
    }
}

impl<C: Codec> From<PreparedPacket<C>> for PacketWriteBurst<C> {
    fn from(value: PreparedPacket<C>) -> Self {
        Self::Mono(value)
    }
}

impl<C: Codec, T: IntoIterator<Item = PreparedPacket<C>>> From<T> for PacketWriteBurst<C> {
    fn from(value: T) -> Self {
        let mut iter = value.into_iter();

        let Some(one) = iter.next() else {
            return Self::Empty;
        };

        let codec_hash = one.hash;
        let Some(two) = iter.next() else {
            return Self::Mono(one);
        };

        if two.hash != codec_hash {
            return Self::ErrHashMismatch;
        }

        let mut vec = Vec::with_capacity(iter.size_hint().0 + 2);
        vec.push(one.data);
        vec.push(two.data);

        for elem in iter {
            if elem.hash != codec_hash {
                return Self::ErrHashMismatch;
            }

            vec.push(elem.data);
        }

        Self::Burst(codec_hash, vec)
    }
}
