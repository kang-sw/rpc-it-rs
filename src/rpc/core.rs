//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::{
    marker::PhantomData,
    num::NonZeroUsize,
    sync::{Arc, Weak},
};

use bytes::Bytes;
use futures::{task::AtomicWaker, AsyncWrite, AsyncWriteExt, Future};

use crate::{
    error::WriteRunnerResult,
    io::{AsyncFrameRead, AsyncFrameWrite},
    NotifySender,
};

use super::{
    error::{ReadRunnerExitType, WriteRunnerError, WriteRunnerExitType},
    req_rep::RequestContext,
    Config, Receiver, RequestSender, WriterDirective,
};

///
pub struct Builder<R: Config, Wr, Rd, U, C> {
    writer: Wr,
    reader: Rd,
    user_data: U,
    codec: C,
    cfg: InitConfig,
    __: std::marker::PhantomData<R>,
}

pub(super) struct RpcCore<R: Config> {
    pub(super) codec: R::Codec,
    user_data: R::UserData,
    reqs: Option<RequestContext>,
    t_tx_deferred: Option<mpsc::Sender<WriterDirective>>,
    r_wake_on_drop: AtomicWaker,
}

/// Non-generic configuration for [`Builder`].
#[derive(Default)]
struct InitConfig {
    /// Channel capacity for deferred directive queue.
    writer_channel_capacity: Option<NonZeroUsize>,
}

// ========================================================== RpcContext ===|

impl<R: Config> RpcCore<R> {
    pub fn user_data(&self) -> &R::UserData {
        &self.user_data
    }

    pub fn tx_deferred(&self) -> Option<&mpsc::Sender<WriterDirective>> {
        self.t_tx_deferred.as_ref()
    }

    pub fn request_context(&self) -> Option<&RequestContext> {
        self.reqs.as_ref()
    }

    pub(super) fn register_wake_on_drop(&self, waker: &std::task::Waker) {
        self.r_wake_on_drop.register(waker);
    }
}

impl<R: Config> std::fmt::Debug for RpcCore<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcCore")
            .field("codec", &self.codec)
            .field("user_data", &self.user_data)
            .field("reqs", &self.reqs)
            .finish()
    }
}

// ========================================================== Builder ===|

pub fn builder<R: Config>() -> Builder<R, (), (), (), ()> {
    Builder {
        writer: (),
        reader: (),
        user_data: (),
        codec: (),
        cfg: InitConfig::default(),
        __: PhantomData,
    }
}

impl<R: Config, Wr, Rd, U, C> Builder<R, Wr, Rd, U, C> {
    // TODO: Add documentation for these methods.

    pub fn with_frame_writer<Wr2>(self, writer: Wr2) -> Builder<R, Wr2, Rd, U, C>
    where
        Wr2: AsyncFrameWrite,
    {
        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            __: PhantomData,
        }
    }

    pub fn with_stream_writer<Wr2>(self, writer: Wr2) -> Builder<R, Wr2, Rd, U, C>
    where
        Wr2: AsyncWrite,
    {
        use std::pin::Pin;

        #[repr(transparent)]
        struct WriterAdapter<Wr2>(Wr2);

        impl<Wr2> WriterAdapter<Wr2> {
            fn as_inner_pinned(self: Pin<&mut Self>) -> Pin<&mut Wr2> {
                // SAFETY: We won't move the inner value anywhere. Its usage is limited.
                unsafe { self.map_unchecked_mut(|x| &mut x.0) }
            }
        }

        impl<Wr2> AsyncFrameWrite for WriterAdapter<Wr2>
        where
            Wr2: 'static + AsyncWrite + Send,
        {
            async fn write_frame<'this>(
                self: Pin<&'this mut Self>,
                buf: Bytes,
            ) -> std::io::Result<()> {
                self.as_inner_pinned().write_all(&buf[..]).await
            }
        }

        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,

            __: PhantomData,
        }
    }

    pub fn with_frame_reader<Rd2>(self, reader: Rd2) -> Builder<R, Wr, Rd2, U, C>
    where
        Rd2: AsyncFrameRead,
    {
        Builder {
            writer: self.writer,
            reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,

            __: PhantomData,
        }
    }

    pub fn with_user_data(self, user_data: R::UserData) -> Builder<R, Wr, Rd, R::UserData, C> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data,
            codec: self.codec,
            cfg: self.cfg,

            __: PhantomData,
        }
    }

    pub fn with_codec<C2>(self, codec: C2) -> Builder<R, Wr, Rd, U, C2> {
        Builder {
            codec,
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            cfg: self.cfg,

            __: PhantomData,
        }
    }

    pub fn with_outbound_queue_capacity(self, capacity: impl TryInto<NonZeroUsize>) -> Self {
        Builder {
            cfg: InitConfig {
                writer_channel_capacity: capacity.try_into().ok(),
            },
            ..self
        }
    }
}

macro_rules! must_use_message {
    () => {
        "futures do nothing unless you `.await` or poll them"
    };
}

impl<R: Config, Wr, Rd> Builder<R, Wr, Rd, R::UserData, R::Codec>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
{
    /// Create a bidirectional RPC connection in client mode; which won't create inbound receiver
    /// channel, but spawns reader task to enable receiving responses.
    #[must_use = must_use_message!()]
    pub fn build_client(
        self,
    ) -> (
        RequestSender<R>,
        impl Future<Output = std::io::Result<ReadRunnerExitType>>,
        impl Future<Output = WriteRunnerResult>,
    ) {
        let (tx_w, rx_w) = self.cfg.make_write_channel();
        let context = Arc::new(RpcCore {
            user_data: self.user_data,
            reqs: Some(RequestContext::new()),
            codec: self.codec,
            t_tx_deferred: Some(tx_w.clone()),
            r_wake_on_drop: Default::default(),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());
        let task_read = Receiver::new(context.clone(), self.reader).into_weak_runner_task(drop);

        (
            RequestSender {
                inner: NotifySender { context },
            },
            task_read,
            task_write,
        )
    }

    /// Create a bidirectional RPC connection in server mode; which will create inbound receiver
    /// channel. If you need sender channel either, you can spawn a sender handle from the receiver.
    #[must_use = must_use_message!()]
    pub fn build_server(
        self,
        enable_request: bool,
    ) -> (Receiver<R, Rd>, impl Future<Output = WriteRunnerResult>) {
        let (tx_w, rx_w) = self.cfg.make_write_channel();
        let context = Arc::new(RpcCore {
            user_data: self.user_data,
            reqs: enable_request.then(RequestContext::new),
            codec: self.codec,
            t_tx_deferred: Some(tx_w.clone()),
            r_wake_on_drop: Default::default(),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());

        (Receiver::new(context, self.reader), task_write)
    }
}

impl<R: Config, Wr, Rd> Builder<R, Wr, Rd, R::UserData, R::Codec>
where
    Rd: AsyncFrameRead,
{
    /// Creates read-only service. The receiver may never take any request.
    #[must_use = must_use_message!()]
    pub fn build_read_only(self) -> Receiver<R, Rd> {
        let Self {
            reader,
            user_data,
            codec,
            ..
        } = self;

        let context = Arc::new(RpcCore {
            user_data,
            codec,
            reqs: None,
            t_tx_deferred: None,
            r_wake_on_drop: Default::default(),
        });

        Receiver::new(context, reader)
    }
}

impl<R: Config, Wr, Rd> Builder<R, Wr, Rd, R::UserData, R::Codec>
where
    Wr: AsyncFrameWrite,
{
    /// Creates write-only client.
    #[must_use = must_use_message!()]
    pub fn build_write_only(
        self,
    ) -> (
        super::NotifySender<R>,
        impl Future<Output = WriteRunnerResult>,
    ) {
        let Self {
            writer,
            user_data,
            codec,
            cfg,
            .. // Read side is not needed.
        } = self;

        let (tx_directive, rx) = cfg.make_write_channel();

        let context = Arc::new(RpcCore {
            user_data,
            codec,
            reqs: None,
            t_tx_deferred: Some(tx_directive),
            r_wake_on_drop: Default::default(),
        });

        (
            NotifySender { context },
            write_runner::<_, R>(writer, rx, Default::default()),
        )
    }
}

impl InitConfig {
    fn make_write_channel(
        &self,
    ) -> (
        mpsc::Sender<WriterDirective>,
        mpsc::Receiver<WriterDirective>,
    ) {
        self.writer_channel_capacity
            .map_or_else(mpsc::unbounded, |x| mpsc::bounded(x.get()))
    }
}

impl<R: Config> Drop for RpcCore<R> {
    fn drop(&mut self) {
        self.r_wake_on_drop.wake();
    }
}

// ========================================================== Runners ===|

async fn write_runner<Wr, R>(
    writer: Wr,
    rx_directive: mpsc::Receiver<WriterDirective>,
    w_ctx: Weak<RpcCore<R>>,
) -> WriteRunnerResult
where
    Wr: AsyncFrameWrite,
    R: Config,
{
    // Implements bulk receive to minimize number of polls on the channel

    let exec_result = async {
        futures::pin_mut!(writer); // Constrain lifetime of `writer` to this block.

        let mut exit_type = WriteRunnerExitType::AllHandleDropped;
        while let Ok(msg) = rx_directive.recv().await {
            match msg {
                WriterDirective::CloseImmediately => {
                    // Prevent further messages from being sent immediately. This is basically
                    // best-effort attempt, which simply neglects remaining messages in the queue.
                    //
                    // This can return false(already closed), where the sender closes the
                    // channel preemptively to block channel as soon as possible.
                    rx_directive.close();
                    writer
                        .as_mut()
                        .close()
                        .await
                        .map_err(WriteRunnerError::WriterCloseFailed)?;

                    return Ok(WriteRunnerExitType::ManualCloseImmediate);
                }
                WriterDirective::CloseAfterFlush => {
                    rx_directive.close(); // Same as above.

                    // To flush rest of the messages, just continue the loop. Since we closed the
                    // channel already, the loop will be terminated soon after consuming all the
                    // remaining messages.
                    exit_type = WriteRunnerExitType::ManualClose;
                }
                WriterDirective::Flush => {
                    writer
                        .as_mut()
                        .flush()
                        .await
                        .map_err(WriteRunnerError::WriterFlushFailed)?;
                }
                WriterDirective::WriteMsg(payload) => {
                    writer
                        .as_mut()
                        .write_frame(payload)
                        .await
                        .map_err(WriteRunnerError::WriteFailed)?;
                }
                WriterDirective::WriteMsgBurst(chunks) => {
                    writer
                        .as_mut()
                        .write_frame_burst(chunks)
                        .await
                        .map_err(WriteRunnerError::WriteFailed)?;
                }
                WriterDirective::WriteReqMsg(payload, req_id) => {
                    // We don't immediately break out of this loop when write ops is
                    // failed; First we have to prevent any awaiting request hang while
                    // not notified the right operation was failed at first place.
                    let write_result = writer
                        .as_mut()
                        .write_frame(payload)
                        .await
                        .map_err(WriteRunnerError::WriteFailed);

                    let Some(context) = w_ctx.upgrade() else {
                        // If all request handles were dropped, it means there's no
                        // awaiting requests that were sent, which makes sending request
                        // and receiving response pointless.

                        continue;
                    };

                    // All path to send request on disabled client should be blocked:
                    // - Notify Sender -> Request Sender upgrade will be blocked if `reqs`
                    //   not present
                    // - Once request sender present -> It'll always be valid
                    let reqs = context
                        .reqs
                        .as_ref()
                        .expect("disabled request feature; logic error!");

                    if let Err(e) = write_result {
                        // At this context, the writer is in broken state.
                        reqs.set_request_write_failed(req_id);
                        return Err(e);
                    }
                }
            }
        }

        if matches!(exit_type, WriteRunnerExitType::ManualCloseImmediate) {
            // We didn't closed the channel yet!
            writer
                .close()
                .await
                .map_err(WriteRunnerError::WriterCloseFailed)?;
        }

        Ok::<_, WriteRunnerError>(exit_type)
    }
    .await;

    // When request feature is enabled ...
    if let Some(reqs) = w_ctx.upgrade().as_ref().and_then(|x| x.reqs.as_ref()) {
        // Assures that the writer channel is closed.
        rx_directive.close();

        // Since any further trials to send requests will fail, we can invalidate all pending
        // requests here safely.
        reqs.mark_expired();
    }

    exec_result
}
