macro_rules! mt_trait {
    ( $trait_name: ty) => {
        paste::paste! {
            pub trait [<Mt $trait_name>]: $trait_name + Unpin + Send + Sync {
                fn adapt(&mut self) -> &mut (dyn $trait_name + Unpin + Send + Sync);
            }

            impl<T: $trait_name + Unpin + Send + Sync> [<Mt $trait_name>] for T {
                fn adapt(&mut self) -> &mut (dyn $trait_name + Unpin + Send + Sync) {
                    self
                }
            }
        }
    };
}

// TODO: Websocket support
#[cfg(any(feature = "tokio", feature = "tokio-full"))]
pub mod ext_tokio {
    #[cfg(feature = "tokio-full")]
    use tokio_full as tokio;

    use std::{pin::Pin, task::Poll};

    use crate::{AsyncFrameRead, AsyncFrameWrite};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    /* ------------------------------------------------------------------------------------------ */
    /*                                      ASYNC RW SUPPORT                                      */
    /* ------------------------------------------------------------------------------------------ */
    pub struct ReadAdapter<T>(pub T);
    pub struct WriteAdapter<T>(pub T);

    impl<T: MtAsyncRead> ReadAdapter<T> {
        pub fn boxed(stream: T) -> Box<Self> {
            Self(stream).into()
        }
    }

    impl<T: MtAsyncWrite> WriteAdapter<T> {
        pub fn boxed(stream: T) -> Box<Self> {
            Self(stream).into()
        }
    }

    mt_trait!(AsyncRead);
    mt_trait!(AsyncWrite);

    impl<T: MtAsyncRead> AsyncFrameRead for ReadAdapter<T> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut buf = ReadBuf::new(buf);

            match (Pin::new(self.0.adapt())).poll_read(cx, &mut buf) {
                Poll::Ready(Ok(_)) => Poll::Ready(Ok(buf.filled().len())),
                Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl<T: MtAsyncWrite> AsyncFrameWrite for WriteAdapter<T> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bufs: &[std::io::IoSlice<'_>],
        ) -> std::task::Poll<std::io::Result<usize>> {
            Pin::new(self.0.adapt()).poll_write_vectored(cx, bufs)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(self.0.adapt()).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(self.0.adapt()).poll_shutdown(cx)
        }
    }
}

#[cfg(feature = "futures")]
pub mod ext_futures {
    use std::{pin::Pin, task::Poll};

    use crate::{AsyncFrameRead, AsyncFrameWrite};
    use futures::{AsyncRead, AsyncWrite};

    /* ------------------------------------------------------------------------------------------ */
    /*                                      ASYNC RW SUPPORT                                      */
    /* ------------------------------------------------------------------------------------------ */
    pub struct ReadAdapter<T>(pub T);
    pub struct WriteAdapter<T>(pub T);

    mt_trait!(AsyncRead);
    mt_trait!(AsyncWrite);

    impl<T: MtAsyncRead> AsyncFrameRead for ReadAdapter<T> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut [u8],
        ) -> Poll<std::io::Result<usize>> {
            (Pin::new(self.0.adapt())).poll_read(cx, buf)
        }
    }

    impl<T: MtAsyncWrite> AsyncFrameWrite for WriteAdapter<T> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bufs: &[std::io::IoSlice<'_>],
        ) -> std::task::Poll<std::io::Result<usize>> {
            Pin::new(self.0.adapt()).poll_write_vectored(cx, bufs)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(self.0.adapt()).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            Pin::new(self.0.adapt()).poll_close(cx)
        }
    }
}
