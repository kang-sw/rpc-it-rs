// TODO: futures AsyncWrite / AsyncRead support

// TODO: TOKIO AsyncWrite / AsyncRead support, TOKIO Tcp support

// TODO: Websocket support

#[cfg(feature = "futures")]
pub mod futures_io {
    use crate::AsyncFrameWrite;

    impl AsyncFrameWrite for dyn futures::AsyncWrite + Send + Sync + Unpin {
        fn begin_write_msg(&mut self) -> std::io::Result<()> {
            todo!()
        }

        fn poll_write_all<'cx, 'b>(
            self: std::pin::Pin<&mut Self>,
            cx: &'cx mut std::task::Context<'cx>,
            bufs: &'b [&'b [u8]],
        ) -> std::task::Poll<std::io::Result<()>> {
            todo!()
        }
    }
}
