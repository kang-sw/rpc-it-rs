//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::{
    borrow::Cow,
    future::poll_fn,
    marker::PhantomData,
    num::NonZeroUsize,
    sync::{Arc, Weak},
    task::Poll,
};

use bytes::Bytes;
use futures::{task::AtomicWaker, AsyncWrite, AsyncWriteExt, Future, FutureExt};

use crate::{
    codec::{Codec, InboundFrameType},
    defs::{NonZeroRangeType, RangeType},
    error::{ReadRunnerResult, WriteRunnerResult},
    io::{AsyncFrameRead, AsyncFrameWrite},
    Inbound, NotifySender,
};

use super::{
    error::{ReadRunnerError, ReadRunnerExitType, WriteRunnerError, WriteRunnerExitType},
    req_rep::RequestContext,
    Config, InboundDelivery, ReceiveErrorHandler, Receiver, RequestSender, WriterDirective,
};

///
pub struct Builder<R: Config, Wr, Rd, U, C, RH> {
    writer: Wr,
    reader: Rd,
    read_event_handler: RH,
    user_data: U,
    codec: C,
    cfg: InitConfig,
    __: std::marker::PhantomData<R>,
}

pub(super) struct RpcCore<R: Config> {
    pub(super) codec: R::Codec,
    user_data: R::UserData,
    reqs: Option<RequestContext<R::Codec>>,
    send_ctx: Option<SenderContext>,
}

struct SenderContext {
    tx_deferred: mpsc::Sender<WriterDirective>,
}

#[derive(Default)]
struct ReceiverContext {
    drop_waker: AtomicWaker,
}

/// Non-generic configuration for [`Builder`].
#[derive(Default)]
struct InitConfig {
    /// Channel capacity for deferred directive queue.
    writer_channel_capacity: Option<NonZeroUsize>,

    /// Maximum queued inbound count.
    inbound_queue_capacity: Option<NonZeroUsize>,
}

// ========================================================== RpcContext ===|

impl<R: Config> RpcCore<R> {
    pub fn user_data(&self) -> &R::UserData {
        &self.user_data
    }

    pub fn tx_deferred(&self) -> Option<&mpsc::Sender<WriterDirective>> {
        self.send_ctx.as_ref().map(|x| &x.tx_deferred)
    }

    pub fn request_context(&self) -> Option<&RequestContext<R::Codec>> {
        self.reqs.as_ref()
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

pub fn builder<R: Config>() -> Builder<R, (), (), (), (), ()> {
    Builder {
        writer: (),
        reader: (),
        read_event_handler: (),
        user_data: (),
        codec: (),
        cfg: InitConfig::default(),
        __: PhantomData,
    }
}

impl<R: Config, Wr, Rd, U, C, RH> Builder<R, Wr, Rd, U, C, RH> {
    // TODO: Add documentation for these methods.

    pub fn with_frame_writer<Wr2>(self, writer: Wr2) -> Builder<R, Wr2, Rd, U, C, RH>
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
            __: PhantomData,
        }
    }

    pub fn with_stream_writer<Wr2>(self, writer: Wr2) -> Builder<R, Wr2, Rd, U, C, RH>
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
            read_event_handler: self.read_event_handler,
            __: PhantomData,
        }
    }

    pub fn with_frame_reader<Rd2>(self, reader: Rd2) -> Builder<R, Wr, Rd2, U, C, RH>
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
            __: PhantomData,
        }
    }

    pub fn with_user_data(self, user_data: R::UserData) -> Builder<R, Wr, Rd, R::UserData, C, RH> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
            __: PhantomData,
        }
    }

    pub fn with_codec<C2>(self, codec: C2) -> Builder<R, Wr, Rd, U, C2, RH> {
        Builder {
            codec,
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            cfg: self.cfg,
            read_event_handler: self.read_event_handler,
            __: PhantomData,
        }
    }

    pub fn with_outbound_queue_capacity(self, capacity: impl TryInto<NonZeroUsize>) -> Self {
        Builder {
            cfg: InitConfig {
                writer_channel_capacity: capacity.try_into().ok(),
                ..self.cfg
            },
            ..self
        }
    }

    pub fn with_inbound_queue_capacity(self, capacity: impl TryInto<NonZeroUsize>) -> Self {
        Builder {
            cfg: InitConfig {
                inbound_queue_capacity: capacity.try_into().ok(),
                ..self.cfg
            },
            ..self
        }
    }

    pub fn with_read_event_handler<RH2>(
        self,
        read_event_handler: RH2,
    ) -> Builder<R, Wr, Rd, U, C, RH2> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
            read_event_handler,
            __: PhantomData,
        }
    }
}

macro_rules! must_use_message {
    () => {
        "futures do nothing unless you `.await` or poll them"
    };
}

impl<R: Config, Wr, Rd, RH> Builder<R, Wr, Rd, R::UserData, R::Codec, RH>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
    RH: ReceiveErrorHandler<R>,
{
    /// Create a bidirectional RPC connection in client mode; which won't create inbound receiver
    /// channel, but spawns reader task to enable receiving responses.
    #[must_use = must_use_message!()]
    pub fn build_client(
        self,
    ) -> (
        RequestSender<R>,
        impl Future<Output = ReadRunnerResult>,
        impl Future<Output = WriteRunnerResult>,
    ) {
        let (tx_w, rx_w) = self.cfg.make_write_channel();
        let context = Arc::new(RpcCore {
            user_data: self.user_data,
            reqs: Some(RequestContext::new(self.codec.fork())),
            codec: self.codec,
            send_ctx: Some(SenderContext {
                tx_deferred: tx_w.clone(),
            }),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());
        let task_read = async move { todo!("receiver.into_response_only_task()") };

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
            reqs: enable_request.then(|| RequestContext::new(self.codec.fork())),
            codec: self.codec,
            send_ctx: Some(SenderContext {
                tx_deferred: tx_w.clone(),
            }),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());

        (Receiver::new(context, self.reader), task_write)
    }
}

impl<R: Config, Wr, Rd, RH> Builder<R, Wr, Rd, R::UserData, R::Codec, RH>
where
    Rd: AsyncFrameRead,
    RH: ReceiveErrorHandler<R>,
{
    /// Creates read-only service. The receiver may never take any request.
    #[must_use = must_use_message!()]
    pub fn build_read_only(self) -> Receiver<R, Rd> {
        let Self {
            reader,
            read_event_handler,
            user_data,
            codec,
            cfg,
            ..
        } = self;

        let context = Arc::new(RpcCore {
            user_data,
            codec,
            reqs: None,
            send_ctx: None,
        });

        Receiver::new(context, reader)
    }
}

impl<R: Config, Wr, Rd, RH> Builder<R, Wr, Rd, R::UserData, R::Codec, RH>
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
            send_ctx: Some(SenderContext {
                tx_deferred: tx_directive.clone(),
            }),
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

    fn make_inbound_channel(
        &self,
    ) -> (
        mpsc::Sender<InboundDelivery>,
        mpsc::Receiver<InboundDelivery>,
    ) {
        self.inbound_queue_capacity
            .map_or_else(mpsc::unbounded, |x| mpsc::bounded(x.get()))
    }
}

// ==== Context Utils ====

impl Drop for ReceiverContext {
    fn drop(&mut self) {
        if let Some(w) = self.drop_waker.take() {
            w.wake();
        }
    }
}

// ========================================================== Runners ===|

#[cfg(any())]
async fn read_runner<Rd, RH, R>(
    reader: Rd,
    handler: RH,
    tx_inbound: Option<mpsc::Sender<InboundDelivery>>,
    wctx: Weak<RpcCore<R>>,
) -> ReadRunnerResult
where
    Rd: AsyncFrameRead,
    RH: ReceiveErrorHandler<R>,
    R: Config,
{
    // - Already expired: This case is not an error!
    // - `ReceiverContext` is missing: this is an error!
    debug_assert!(wctx.upgrade().map(|x| x.recv_ctx.is_some()).unwrap_or(true));

    // Two tasks:
    // - Reads frames from the reader and decode it into inbound message
    //   - During handling, when error occurs; it should be reported to the handler.
    // - Awaits finish signal of rpc context.

    let task_await_finish = poll_fn(|cx| {
        let Some(context) = wctx.upgrade() else {
            return Poll::Ready(());
        };

        context.unwrap_recv().drop_waker.register(cx.waker());
        Poll::Pending
    });

    let task_read_loop = async {
        let mut handler = handler;
        let mut opt_tx_inbound = tx_inbound;
        futures::pin_mut!(reader);

        loop {
            match reader.as_mut().next().await {
                Ok(Some(bytes)) => {
                    let Some(ctx) = wctx.upgrade() else {
                        break Ok(ReadRunnerExitType::AllHandleDropped);
                    };

                    let ib = match ctx.codec.decode_inbound(&bytes) {
                        Ok(ib) => ib,
                        Err(e) => {
                            handler.on_inbound_decode_error(
                                &ctx.user_data,
                                &ctx.codec,
                                &bytes,
                                e,
                            )?;
                            continue;
                        }
                    };

                    let delivery = match ib {
                        InboundFrameType::Request {
                            req_id_raw,
                            method,
                            params,
                        } => InboundDelivery::new_request(bytes, method.into(), params.into(), {
                            NonZeroRangeType::new(
                                req_id_raw.start,
                                req_id_raw.end.try_into().unwrap(),
                            )
                        }),
                        InboundFrameType::Notify { method, params } => {
                            InboundDelivery::new_notify(bytes, method.into(), params.into())
                        }
                        InboundFrameType::Response {
                            req_id,
                            errc,
                            payload,
                        } => {
                            let Some(reqs) = ctx.reqs.as_ref() else {
                                handler.on_impossible_response_inbound(&ctx.user_data)?;
                                continue;
                            };

                            let data = bytes.slice(RangeType::from(payload).range());
                            if !reqs.set_response(req_id, data, errc) {
                                // XXX: Do we need 'handler.on_BLAHBLAH'?
                                // - Would forwarding request ID to user be meaningful?
                                // - Would forwarding request payload to user be meaningful?
                                // - Should the user know that the request was already expired?
                            }

                            continue;
                        }
                    };

                    if std::str::from_utf8(delivery.method_bytes()).is_err() {
                        // Method name *MUST* be valid UTF-8 string.
                        //
                        // XXX: Should we remove this constraint, allowing arbitrary bytes?
                        handler.on_inbound_method_invalid_utf8(
                            &ctx.user_data,
                            delivery.method_bytes(),
                        )?;
                        continue;
                    }

                    let unhandled_delivery = if let Some(tx) = opt_tx_inbound.as_ref() {
                        if let Err(delivery) = tx.send(delivery).await {
                            // Here, all receiver handles were dropped.

                            if ctx.reqs.as_ref().is_some_and(|x| !x.is_expired()) {
                                // Still, we have to deal with response inbounds
                                opt_tx_inbound = None;
                                delivery.0
                            } else {
                                // Request feature is already disabled,
                                break Ok(ReadRunnerExitType::AllHandleDropped);
                            }
                        } else {
                            continue;
                        }
                    } else {
                        // From the very next loop of `tx_inbound` drop, all inbound delivery will
                        // be forwarded to user registered handler.
                        delivery
                    };

                    handler.on_unhandled_notify_or_request(
                        &ctx.user_data,
                        Inbound::new(ctx.clone(), unhandled_delivery),
                    )?;
                }
                Ok(None) => break Ok(ReadRunnerExitType::AllHandleDropped),
                Err(io_error) => break Err(ReadRunnerError::from(io_error)),
            }
        }
    };

    let result = futures::select_biased! {
        r = task_read_loop.fuse() => r,
        _ = task_await_finish.fuse() => Ok(ReadRunnerExitType::AllHandleDropped),
    };

    if let Some(ctx) = wctx.upgrade().as_deref().and_then(|x| x.reqs.as_ref()) {
        // The reader channel was expired, but seems there's still alive senders who can send
        // request. Therefore, we have to explcitly mark that request feature is unavailable anymore
        // as the receiver runner is being terminated.
        ctx.mark_expired();
    }

    result
}

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
                    let write_result = writer
                        .as_mut()
                        .write_frame(payload)
                        .await
                        .map_err(WriteRunnerError::WriteFailed);

                    let Some(context) = w_ctx.upgrade() else {
                        // If all request handles were dropped, it means there's no awaiting
                        // requests that were sent, which makes sending request and receiving
                        // response pointless.

                        continue;
                    };

                    // All path to send request on disabled client should be blocked:
                    // - Notify Sender -> Request Sender upgrade will be blocked if `reqs` not
                    //   present
                    // - Once request sender present -> It'll always be valid
                    let reqs = context
                        .reqs
                        .as_ref()
                        .expect("disabled request feature; logic error!");

                    if let Err(e) = write_result {
                        reqs.invalidate_request(req_id);
                        return Err(e);
                    }
                }
            }
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
