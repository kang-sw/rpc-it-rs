use std::sync::Arc;

use crate::prelude::Codec;

use async_lock::Mutex as AsyncMutex;

/// Creates RPC connection from [`crate::transport::AsyncReadFrame`] and
/// [`crate::transport::AsyncWriteFrame`], and [`crate::codec::Codec`].
///
/// For unsupported features(e.g. notify from client), the codec should return
/// [`crate::codec::EncodeError::UnsupportedFeature`] error.
pub struct ConnectionBody<C, T> {
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
struct RequestContext {}

/// Actual handle to the RPC connection control.
#[derive(Clone, Debug)]
pub struct Handle {
    connection: Arc<dyn Connection>,
}

mod driver {}
