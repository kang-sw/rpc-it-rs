use std::{ops::Range, sync::Arc};

use crate::{codec::DecodeError, prelude::Codec};

use async_lock::Mutex as AsyncMutex;
use smallvec::SmallVec;

/// Creates RPC connection from [`crate::transport::AsyncReadFrame`] and
/// [`crate::transport::AsyncWriteFrame`], and [`crate::codec::Codec`].
///
/// For unsupported features(e.g. notify from client), the codec should return
/// [`crate::codec::EncodeError::UnsupportedFeature`] error.
struct ConnectionBody<C, T> {
    codec: C,
    write: AsyncMutex<T>,
    reqs: RequestContext,
}

trait Connection: Send + Sync + 'static + std::fmt::Debug {
    fn codec(&self) -> &dyn Codec;
    fn write(&self) -> &AsyncMutex<dyn crate::transport::AsyncWriteFrame>;
    fn reqs(&self) -> &RequestContext;
}

impl<C, T> Connection for ConnectionBody<C, T>
where
    C: Codec + Send + Sync + 'static,
    T: crate::transport::AsyncWriteFrame + Send + Sync + 'static,
{
    fn codec(&self) -> &dyn Codec {
        &self.codec
    }

    fn write(&self) -> &AsyncMutex<dyn crate::transport::AsyncWriteFrame> {
        &self.write
    }

    fn reqs(&self) -> &RequestContext {
        &self.reqs
    }
}

impl<C, T> std::fmt::Debug for ConnectionBody<C, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionBody").field("reqs", &self.reqs).finish()
    }
}

/// RPC request context. Stores request ID and response receiver context.
#[derive(Debug)]
struct RequestContext {
    // TODO: Registered request futures ...
    // TODO: Handle cancellations ...
}

/// Handle to the internal RPC connection control. Can be shared among multiple threads and accessed
/// simultaneously.
#[derive(Clone, Debug)]
pub struct Handle(Arc<dyn Connection>);

/// Reused buffer over multiple RPC request/responses
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    keygen_buf: SmallVec<[u8; 36]>,
    tmpbuf: Vec<u8>,
}

impl Handle {
    pub async fn send_request<'a, T: serde::Serialize, R>(
        &'a self,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture<R>, SendError> {
        self.send_request_ex(&mut Default::default(), method, params).await
    }

    pub async fn send_notify<T: serde::Serialize>(
        &self,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        self.send_notify_ex(&mut Default::default(), method, params).await
    }

    #[doc(hidden)]
    pub async fn send_request_ex<'a, T: serde::Serialize, R>(
        &'a self,
        buf_reuse: &mut Buffer,
        method: &str,
        params: &T,
    ) -> Result<ResponseFuture<R>, SendError> {
        todo!()
    }

    #[doc(hidden)]
    pub async fn send_notify_ex<T: serde::Serialize>(
        &self,
        buf_reuse: &mut Buffer,
        method: &str,
        params: &T,
    ) -> Result<(), SendError> {
        todo!()
    }

    pub async fn recv_msg(&self) -> Result<msg::RecvMsg, RecvError> {
        todo!()
    }
}

/// For response, there is no specific 'timeout' on waiting. As soon as we drop this future, the
/// response registration also be unregistered from queue.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ResponseFuture<T> {
    _type: std::marker::PhantomData<T>,
    handle: Handle,
    req_id: u64,
}

#[derive(Debug)]
struct InboundBody {
    buffer: bytes::Bytes,
    handle: Handle,
    payload: Range<usize>,
}

pub trait Inbound: Sized {
    #[doc(hidden)]
    fn inbound_get_handle(&self) -> &Handle;

    fn method(&self) -> Option<&str>;
    fn payload(&self) -> &[u8];

    fn decode_in_place<'a, T: serde::Deserialize<'a>>(
        &'a self,
        dst: &mut Option<T>,
    ) -> Result<(), DecodeError> {
        if let Some(dst) = dst.as_mut() {
        } else {
            self.inbound_get_handle().0.codec().decode_payload(self.payload(), &mut |de| {
                *dst = Some(erased_serde::deserialize(de)?);
                Ok(())
            })?;
        }

        Ok(())
    }

    fn decode<'a, T: serde::Deserialize<'a>>(&'a self) -> Result<T, DecodeError> {
        let mut dst = None;
        self.decode_in_place(&mut dst)?;
        Ok(dst.unwrap())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("Encoding outbound message failed: {0}")]
    CodecError(#[from] crate::codec::EncodeError),

    #[error("Error during preparing send: {0}")]
    SendSetupFailed(std::io::Error),

    #[error("Error during sending encoded message: {0}")]
    SendFailed(std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    // TODO: Connection Closed, Aborted, Parse Error, etc...
}

// TODO: Request Builder
//  - Create async worker task to handle receive

pub mod msg {
    use std::ops::Range;

    #[derive(Debug)]
    pub struct Request {
        h: super::InboundBody,
        method: Range<usize>,
        req_id: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Notify {
        h: super::InboundBody,
        method: Range<usize>,
    }

    #[derive(Debug)]
    pub struct Response {
        h: super::InboundBody,
        is_error: bool,
    }

    #[derive(Debug)]
    pub enum RecvMsg {
        Request(Request),
        Notify(Notify),
    }

    // TODO: Inbound impls
}

mod driver {
    //! Internal driver context. It receives the request from the connection, and dispatches it to
    //! the handler.
    //!
    // TODO: Driver implementation
}
