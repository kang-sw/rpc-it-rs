use rpc_it::AsyncFrameWrite;

pub struct WriteAdapter(pub mpsc::Sender<bytes::Bytes>);

impl AsyncFrameWrite for WriteAdapter {
    fn poll_write_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut bytes::Bytes,
    ) -> std::task::Poll<std::io::Result<()>> {
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.0) };

        todo!()
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        todo!()
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        todo!()
    }
}
