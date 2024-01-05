//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::{future::poll_fn, num::NonZeroUsize, sync::Arc};

use futures::Future;
use tokio::sync::mpsc;

use crate::{
    codec::Codec,
    io::{AsyncFrameRead, AsyncFrameWrite},
    NotifySender,
};

use super::{
    error::{WriteRunnerError, WriteRunnerExitType},
    req_rep::RequestContext,
    DeferredDirective, RpcContext, UserData,
};

/// Handles error during receiving inbound messages inside runner.
pub trait ReceiveErrorHandler {}

///
pub struct Builder<Wr, Rd, U, C> {
    writer: Wr,
    reader: Rd,
    user_data: U,
    codec: C,
    cfg: InitConfig,
}

struct RpcContextImpl<U, C> {
    user_data: U,
    codec: C,
    r: Option<ReceiverContext>,
}

struct ReceiverContext {}

/// Non-generic configuration for [`Builder`].
#[derive(Default)]
struct InitConfig {
    /// Channel capacity for deferred directive queue.
    writer_channel_capacity: usize,
}

// ========================================================== RpcContext ===|

impl<U, C> RpcContext<U> for RpcContextImpl<U, C>
where
    U: UserData,
    C: Codec,
{
    fn self_as_codec(self: Arc<Self>) -> Arc<dyn Codec> {
        todo!()
    }

    fn codec(&self) -> &dyn Codec {
        todo!()
    }

    fn user_data(&self) -> &U {
        todo!()
    }

    fn shutdown_rx_channel(&self) {
        todo!()
    }

    fn incr_request_sender_refcnt(&self) {
        todo!()
    }

    fn decr_request_sender_refcnt(&self) {
        todo!()
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

pub fn create_builder() -> Builder<(), (), (), ()> {
    Builder {
        writer: (),
        reader: (),
        user_data: (),
        codec: (),
        cfg: InitConfig::default(),
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C> {
    pub fn with_writer<Wr2>(self, writer: Wr2) -> Builder<Wr2, Rd, U, C>
    where
        Wr2: AsyncFrameWrite,
    {
        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_reader<Rd2>(self, reader: Rd2) -> Builder<Wr, Rd2, U, C>
    where
        Rd2: AsyncFrameRead,
    {
        Builder {
            writer: self.writer,
            reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_user_data<U2>(self, user_data: U2) -> Builder<Wr, Rd, U2, C> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_codec<C2>(self, codec: C2) -> Builder<Wr, Rd, U, C2> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            codec,
            cfg: self.cfg,
        }
    }

    pub fn with_write_channel_capacity(self, capacity: NonZeroUsize) -> Self {
        Builder {
            cfg: InitConfig {
                writer_channel_capacity: capacity.get(),
                ..self.cfg
            },
            ..self
        }
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
{
    /// Creates client.
    #[must_use = "The client will not run unless you spawn task manually"]
    pub fn build(self) -> (super::RequestSender<U>, super::Receiver<U>, impl Future) {
        let runner = async {};

        (todo!(), todo!(), runner)
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C>
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
            reader: _, // It's unused!
            user_data,
            codec,
            cfg,
        } = self;

        let (tx_directive, rx) = mpsc::channel(cfg.writer_channel_capacity);
        let context = Arc::new(RpcContextImpl {
            user_data,
            codec,
            r: None,
        });

        (
            NotifySender {
                context,
                tx_deferred: tx_directive,
            },
            write_runner(writer, rx, None),
        )
    }
}

// ========================================================== Runners ===|

async fn write_runner<Wr>(
    writer: Wr,
    mut rx_directive: mpsc::Receiver<DeferredDirective>,
    reqs: Option<Arc<RequestContext>>,
) -> Result<WriteRunnerExitType, WriteRunnerError>
where
    Wr: AsyncFrameWrite,
{
    tokio::pin!(writer);

    // Implements bulk receive to minimize number of polls on the channel

    let exec_result = async {
        let mut exit_type = WriteRunnerExitType::AllHandleDropped;
        while let Some(msg) = rx_directive.recv().await {
            match msg {
                DeferredDirective::CloseImmediately => {
                    // Prevent further messages from being sent immediately. This is basically
                    // best-effort attempt, which simply neglects remaining messages in the queue.
                    rx_directive.close();

                    poll_fn(|cx| writer.as_mut().poll_close(cx))
                        .await
                        .map_err(WriteRunnerError::WriterCloseFailed)?;

                    return Ok(WriteRunnerExitType::ManualCloseImmediate);
                }
                DeferredDirective::CloseAfterFlush => {
                    rx_directive.close(); // Simply prevents further messages from being sent

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
                DeferredDirective::WriteNoti(mut payload) => {
                    poll_fn(|cx| writer.as_mut().poll_write_frame(cx, &mut payload))
                        .await
                        .map_err(WriteRunnerError::WriteFailed)?;
                }
                DeferredDirective::WriteReq(mut payload, req_id) => {
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
