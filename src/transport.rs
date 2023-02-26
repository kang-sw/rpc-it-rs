use std::pin::Pin;

///
/// A message writer trait, which is used to write message to the underlying transport
/// layer.
///
pub trait AsyncFrameWrite: Send + Sync + Unpin {
    /// Called before writing a message. This should clean all internal in-progress message
    /// transport state.
    fn begin_write_msg(&mut self) -> std::io::Result<()>;

    /// Returns OK only when all the requested data is written to the underlying transport
    /// layer.
    fn poll_write_all<'cx, 'b>(
        self: Pin<&mut Self>,
        cx: &'cx mut std::task::Context<'cx>,
        bufs: &'b [&'b [u8]],
    ) -> std::task::Poll<std::io::Result<()>>;
}

///
/// A message reader trait. This is used to read message from the underlying transport layer.
///
pub trait AsyncFrameRead: Send + Sync + Unpin {
    /// Called before reading a message. This should clean all internal in-progress message
    /// transport state.
    fn begin_read_msg(&mut self) -> std::io::Result<()>;

    /// Returns OK only when all the requested data is read from the underlying transport.
    ///
    /// Unless the driver instance is disposed of after this method is called,
    /// the [`AsyncFrameRead::poll_read_body`] method is guaranteed to be called before
    /// next [`AsyncFrameRead::begin_read_msg`] call.
    ///
    /// Argument `buf` is likely to be the same as the size of [`crate::raw::MessageHeaderRaw`]
    ///
    /// FrameReceive's incoming polling logic is intentionally split in two, to allow
    /// implementations that receive incoming packets that have already been framed,
    /// such as WebSocket or UDP, and in a subsequent call to [`AsyncFrameRead::poll_read_body`]
    /// simply dump the already received packets into the output buffer.
    fn poll_read_head<'this, 'cx, 'b: 'this>(
        self: Pin<&'this mut Self>,
        cx: &'cx mut std::task::Context<'cx>,
        buf: &'b mut [u8],
    ) -> std::task::Poll<std::io::Result<()>>;

    /// Returns Ok when enough bytes have been received to exactly fill the requested `buf`
    /// parameter. This function should always return [`std::task::Poll::Pending`] until
    /// the buffer is full, unless the connection is interrupted.
    ///
    /// The lifetime of the supplied buffer argument is guaranteed to be longer than the
    /// lifetime of the instance of this trait object.
    fn poll_read_body<'this, 'cx, 'b: 'this>(
        self: Pin<&'this mut Self>,
        cx: &'cx mut std::task::Context<'cx>,
        buf: &'b mut [u8],
    ) -> std::task::Poll<std::io::Result<()>>;
}
