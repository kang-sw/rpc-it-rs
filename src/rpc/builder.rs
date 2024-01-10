//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::{future::poll_fn, num::NonZeroUsize, sync::Arc};

use bytes::{Buf, Bytes};
use futures::{AsyncWrite, Future};

use crate::{
    codec::Codec,
    io::{AsyncFrameRead, AsyncFrameWrite},
    NotifySender,
};

use super::{
    error::{ReadRunnerError, ReadRunnerExitType, WriteRunnerError, WriteRunnerExitType},
    req_rep::RequestContext,
    DeferredDirective, InboundDelivery, ReceiveErrorHandler, RpcCore, UserData,
};

///
pub struct Builder<Wr, Rd, U, C, RH> {
    writer: Wr,
    reader: Rd,
    read_event_handler: RH,
    user_data: U,
    codec: C,
    cfg: InitConfig,
}

struct RpcContextImpl<U, C> {
    user_data: U,
    codec: C,
    t: Option<SenderContext>,
    r: Option<ReceiverContext>,
}

struct SenderContext {
    tx_deferred: mpsc::Sender<DeferredDirective>,
}

struct ReceiverContext {}

/// Non-generic configuration for [`Builder`].
#[derive(Default)]
struct InitConfig {
    /// Channel capacity for deferred directive queue.
    writer_channel_capacity: Option<NonZeroUsize>,

    /// Maximum queued inbound count.
    inbound_queue_capacity: Option<NonZeroUsize>,
}

// ========================================================== RpcContext ===|

impl<U, C> RpcCore<U> for RpcContextImpl<U, C>
where
    U: UserData,
    C: Codec,
{
    fn self_as_codec(self: Arc<Self>) -> Arc<dyn Codec> {
        self
    }

    fn codec(&self) -> &dyn Codec {
        self
    }

    fn user_data(&self) -> &U {
        &self.user_data
    }

    fn shutdown_rx_channel(&self) {
        todo!("Send shutdown signal to the channel")
    }

    fn tx_deferred(&self) -> Option<&mpsc::Sender<DeferredDirective>> {
        self.t.as_ref().map(|x| &x.tx_deferred)
    }

    fn on_request_unhandled(&self, req_id: &[u8]) {
        todo!("Tries to send `unhandled` error response")
    }
}

impl<U, C> Codec for RpcContextImpl<U, C>
where
    C: Codec,
    U: UserData,
{
    fn encode_notify(
        &self,
        method: &str,
        params: &dyn erased_serde::Serialize,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        self.codec.encode_notify(method, params, buf)
    }

    fn encode_request(
        &self,
        request_id: crate::defs::RequestId,
        method: &str,
        params: &dyn erased_serde::Serialize,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        self.codec.encode_request(request_id, method, params, buf)
    }

    fn encode_response(
        &self,
        request_id_raw: &[u8],
        result: crate::codec::EncodeResponsePayload,
        buf: &mut bytes::BytesMut,
    ) -> Result<(), crate::codec::error::EncodeError> {
        self.codec.encode_response(request_id_raw, result, buf)
    }

    fn deserialize_payload(
        &self,
        payload: &[u8],
        visitor: &mut dyn FnMut(
            &mut dyn erased_serde::Deserializer,
        ) -> Result<(), erased_serde::Error>,
    ) -> Result<(), erased_serde::Error> {
        self.codec.deserialize_payload(payload, visitor)
    }
}

impl<U, C> std::fmt::Debug for RpcContextImpl<U, C>
where
    U: UserData,
    C: Codec,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcContextImpl")
            .field("user_data", &self.user_data)
            .field("codec", &self.codec)
            .field("request_enabled", &self.r.is_some())
            .finish()
    }
}

// ========================================================== Builder ===|

pub fn create_builder() -> Builder<(), (), (), (), ()> {
    Builder {
        writer: (),
        reader: (),
        read_event_handler: (),
        user_data: (),
        codec: (),
        cfg: InitConfig::default(),
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH> {
    // TODO: Add documentation for these methods.

    pub fn with_frame_writer<Wr2>(self, writer: Wr2) -> Builder<Wr2, Rd, U, C, RH>
    where
        Wr2: AsyncFrameWrite,
    {
        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
        }
    }

    pub fn with_stream_writer<Wr2>(self, writer: Wr2) -> Builder<Wr2, Rd, U, C, RH>
    where
        Wr2: AsyncWrite,
    {
        use std::pin::Pin;
        use std::task::{Context, Poll};

        #[repr(transparent)]
        struct WriterAdapter<Wr2>(Wr2);

        impl<Wr2> WriterAdapter<Wr2> {
            fn as_inner_pinned(self: Pin<&mut Self>) -> Pin<&mut Wr2> {
                unsafe { self.map_unchecked_mut(|x| &mut x.0) }
            }
        }

        impl<Wr2> AsyncFrameWrite for WriterAdapter<Wr2>
        where
            Wr2: 'static + AsyncWrite + Send,
        {
            fn poll_write_frame(
                self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut Bytes,
            ) -> Poll<std::io::Result<()>> {
                let mut this = self.as_inner_pinned();

                while !buf.is_empty() {
                    match futures::ready!(this.as_mut().poll_write(cx, buf))? {
                        0 => {
                            return Poll::Ready(Err(std::io::Error::from(
                                std::io::ErrorKind::WriteZero,
                            )))
                        }
                        nwrite => buf.advance(nwrite),
                    }
                }

                Poll::Ready(Ok(()))
            }

            fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.as_inner_pinned().poll_flush(cx)
            }

            fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                self.as_inner_pinned().poll_close(cx)
            }
        }

        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
        }
    }

    pub fn with_frame_reader<Rd2>(self, reader: Rd2) -> Builder<Wr, Rd2, U, C, RH>
    where
        Rd2: AsyncFrameRead,
    {
        Builder {
            writer: self.writer,
            reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
        }
    }

    pub fn with_user_data<U2>(self, user_data: U2) -> Builder<Wr, Rd, U2, C, RH> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
        }
    }

    pub fn with_codec<C2>(self, codec: C2) -> Builder<Wr, Rd, U, C2, RH> {
        Builder {
            codec,
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
        }
    }

    pub fn with_write_channel_capacity(self, capacity: NonZeroUsize) -> Self {
        Builder {
            cfg: InitConfig {
                writer_channel_capacity: Some(capacity),
                ..self.cfg
            },
            ..self
        }
    }

    pub fn with_inbound_queue_capacity(self, capacity: NonZeroUsize) -> Self {
        Builder {
            cfg: InitConfig {
                inbound_queue_capacity: Some(capacity),
                ..self.cfg
            },
            ..self
        }
    }

    pub fn with_read_event_handler<RH2>(
        self,
        read_event_handler: RH2,
    ) -> Builder<Wr, Rd, U, C, RH2> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler,
        }
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
    RH: ReceiveErrorHandler<U>,
{
    /// Creates client.
    #[must_use = "The client will not run unless you spawn task manually"]
    pub fn build(self) -> (super::RequestSender<U>, super::Receiver<U>, impl Future) {
        let _runner = async {};

        (todo!(), todo!(), _runner)
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
    RH: ReceiveErrorHandler<U>,
{
    /// Creates read-only service. The receiver may never take any request.
    #[must_use = "The client will not run unless you spawn task manually"]
    pub fn build_read_only(self) -> (super::Receiver<U>, impl Future) {
        let _runner = async {};

        (todo!(), _runner)
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Wr: AsyncFrameWrite,
    U: UserData,
    C: Codec,
{
    /// Creates write-only client.
    #[must_use = "The client will not run unless you spawn task manually"]
    pub fn build_write_only(
        self,
    ) -> (
        super::NotifySender<U>,
        impl Future<Output = Result<WriteRunnerExitType, WriteRunnerError>>,
    ) {
        let Self {
            writer,
            user_data,
            codec,
            cfg,
            .. // Read side is not needed.
        } = self;

        let (tx_directive, rx) = cfg
            .writer_channel_capacity
            .map_or_else(mpsc::unbounded, |x| mpsc::bounded(x.get()));

        let context = Arc::new(RpcContextImpl {
            user_data,
            codec,
            t: Some(SenderContext {
                tx_deferred: tx_directive.clone(),
            }),
            r: None,
        });

        (NotifySender { context }, write_runner(writer, rx, None))
    }
}

// ========================================================== Runners ===|

async fn read_runner<Rd, RH, C, U>(
    reader: Rd,
    tx_inbound: mpsc::Sender<InboundDelivery>,
) -> Result<ReadRunnerExitType, ReadRunnerError>
where
    Rd: AsyncFrameRead,
    RH: ReceiveErrorHandler<U>,
    C: Codec,
    U: UserData,
{
    todo!()
}

async fn write_runner<Wr>(
    writer: Wr,
    rx_directive: mpsc::Receiver<DeferredDirective>,
    reqs: Option<Arc<RequestContext>>,
) -> Result<WriteRunnerExitType, WriteRunnerError>
where
    Wr: AsyncFrameWrite,
{
    futures::pin_mut!(writer);

    // Implements bulk receive to minimize number of polls on the channel

    let exec_result = async {
        let mut exit_type = WriteRunnerExitType::AllHandleDropped;
        while let Ok(msg) = rx_directive.recv().await {
            match msg {
                DeferredDirective::CloseImmediately => {
                    // Prevent further messages from being sent immediately. This is basically
                    // best-effort attempt, which simply neglects remaining messages in the queue.
                    //
                    // This can return false(already closed), where the sender closes the
                    // channel preemptively to block channel as soon as possible.
                    rx_directive.close();

                    poll_fn(|cx| writer.as_mut().poll_close(cx))
                        .await
                        .map_err(WriteRunnerError::WriterCloseFailed)?;

                    return Ok(WriteRunnerExitType::ManualCloseImmediate);
                }
                DeferredDirective::CloseAfterFlush => {
                    rx_directive.close(); // Same as above.

                    // To flush rest of the messages, just continue the loop. Since we closed the
                    // channel already, the loop will be terminated soon after consuming all the
                    // remaining messages.
                    exit_type = WriteRunnerExitType::ManualClose;
                }
                DeferredDirective::Flush => {
                    poll_fn(|cx| writer.as_mut().poll_flush(cx))
                        .await
                        .map_err(WriteRunnerError::WriterFlushFailed)?;
                }
                DeferredDirective::WriteMsg(mut payload) => {
                    poll_fn(|cx| writer.as_mut().poll_write_frame(cx, &mut payload))
                        .await
                        .map_err(WriteRunnerError::WriteFailed)?;
                }
                DeferredDirective::WriteReqMsg(mut payload, req_id) => {
                    debug_assert!(reqs.is_some(), "Write only client cannot send requests");

                    let write_result =
                        poll_fn(|cx| writer.as_mut().poll_write_frame(cx, &mut payload))
                            .await
                            .map_err(WriteRunnerError::WriteFailed);

                    if let Err(e) = write_result {
                        reqs.as_deref().unwrap().invalidate_request(req_id);
                        return Err(e);
                    }
                }
            }
        }

        Ok::<_, WriteRunnerError>(exit_type)
    }
    .await;

    // When request feature is enabled ...
    if let Some(reqs) = reqs {
        // Assures that the writer channel is closed.
        rx_directive.close();

        // Since any further trials to send requests will fail, we can invalidate all pending
        // requests here safely.
        reqs.invalidate_all_requests();
    }

    exec_result
}
