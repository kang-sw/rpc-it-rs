//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::{
    borrow::Cow,
    future::poll_fn,
    num::NonZeroUsize,
    sync::{Arc, Weak},
    task::Poll,
};

use bytes::{Buf, Bytes};
use futures::{task::AtomicWaker, AsyncWrite, Future, FutureExt};

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
    DeferredDirective, InboundDelivery, ReceiveErrorHandler, Receiver, RequestSender, UserData,
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

pub(super) struct RpcCore<U, C> {
    user_data: U,
    codec: C,
    reqs: Option<RequestContext<C>>,
    send_ctx: Option<SenderContext>,
    recv_ctx: Option<ReceiverContext>,
}

struct SenderContext {
    tx_deferred: mpsc::Sender<DeferredDirective>,
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

impl<U, C> RpcCore<U, C> {
    pub fn codec(&self) -> &C {
        &self.codec
    }

    pub fn user_data(&self) -> &U {
        &self.user_data
    }

    pub fn tx_deferred(&self) -> Option<&mpsc::Sender<DeferredDirective>> {
        self.send_ctx.as_ref().map(|x| &x.tx_deferred)
    }

    pub fn request_context(&self) -> Option<&RequestContext<C>> {
        self.reqs.as_ref()
    }
}

impl<U, C> std::fmt::Debug for RpcCore<U, C>
where
    U: UserData,
    C: Codec,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcContextImpl")
            .field("user_data", &self.user_data)
            .field("codec", &self.codec)
            .field("request_enabled", &self.recv_ctx.is_some())
            .finish()
    }
}

// ========================================================== Builder ===|

pub fn builder() -> Builder<(), (), (), (), ()> {
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
        use std::task::Context;

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

macro_rules! must_use_message {
    () => {
        "futures do nothing unless you `.await` or poll them"
    };
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
    RH: ReceiveErrorHandler<U, C>,
{
    /// Create a bidirectional RPC connection in client mode; which won't create inbound receiver
    /// channel, but spawns reader task to enable receiving responses.
    #[must_use = must_use_message!()]
    pub fn build_client(
        self,
    ) -> (
        RequestSender<U, C>,
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
            recv_ctx: Some(ReceiverContext::default()),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());
        let task_read = read_runner(self.reader, self.read_event_handler, None, w_context);

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
    ) -> (
        Receiver<U, C>,
        impl Future<Output = ReadRunnerResult>,
        impl Future<Output = WriteRunnerResult>,
    ) {
        let (tx_w, rx_w) = self.cfg.make_write_channel();
        let (tx_ib, rx_ib) = self.cfg.make_inbound_channel();
        let context = Arc::new(RpcCore {
            user_data: self.user_data,
            reqs: enable_request.then(|| RequestContext::new(self.codec.fork())),
            codec: self.codec,
            send_ctx: Some(SenderContext {
                tx_deferred: tx_w.clone(),
            }),
            recv_ctx: Some(ReceiverContext::default()),
        });

        let w_context = Arc::downgrade(&context);
        let task_write = write_runner(self.writer, rx_w, w_context.clone());
        let task_read = read_runner(self.reader, self.read_event_handler, Some(tx_ib), w_context);

        (
            Receiver {
                context,
                channel: rx_ib,
            },
            task_read,
            task_write,
        )
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
    RH: ReceiveErrorHandler<U, C>,
{
    /// Creates read-only service. The receiver may never take any request.
    #[must_use = must_use_message!()]
    pub fn build_read_only(self) -> (Receiver<U, C>, impl Future) {
        let Self {
            reader,
            read_event_handler,
            user_data,
            codec,
            cfg,
            ..
        } = self;

        let (tx_ib, rx_ib) = cfg.make_inbound_channel();
        let context = Arc::new(RpcCore {
            user_data,
            codec,
            reqs: None,
            send_ctx: None,
            recv_ctx: Some(Default::default()),
        });

        let task = read_runner(
            reader,
            read_event_handler,
            Some(tx_ib),
            Arc::downgrade(&context),
        );

        (
            Receiver {
                context,
                channel: rx_ib,
            },
            task,
        )
    }
}

impl<Wr, Rd, U, C, RH> Builder<Wr, Rd, U, C, RH>
where
    Wr: AsyncFrameWrite,
    U: UserData,
    C: Codec,
{
    /// Creates write-only client.
    #[must_use = must_use_message!()]
    pub fn build_write_only(
        self,
    ) -> (
        super::NotifySender<U, C>,
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
            recv_ctx: None,
        });

        (
            NotifySender { context },
            write_runner::<_, U, C>(writer, rx, Default::default()),
        )
    }
}

impl InitConfig {
    fn make_write_channel(
        &self,
    ) -> (
        mpsc::Sender<DeferredDirective>,
        mpsc::Receiver<DeferredDirective>,
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

impl<U, C> RpcCore<U, C>
where
    U: UserData,
    C: Codec,
{
    fn unwrap_recv(&self) -> &ReceiverContext {
        self.recv_ctx.as_ref().unwrap()
    }
}

impl Drop for ReceiverContext {
    fn drop(&mut self) {
        if let Some(w) = self.drop_waker.take() {
            w.wake();
        }
    }
}

// ========================================================== Runners ===|

async fn read_runner<Rd, RH, C, U>(
    reader: Rd,
    handler: RH,
    tx_inbound: Option<mpsc::Sender<InboundDelivery>>,
    wctx: Weak<RpcCore<U, C>>,
) -> ReadRunnerResult
where
    Rd: AsyncFrameRead,
    RH: ReceiveErrorHandler<U, C>,
    C: Codec,
    U: UserData,
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
            match poll_fn(|cx| reader.as_mut().poll_read_frame(cx)).await {
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
                        Inbound::new(Cow::Borrowed(&ctx), unhandled_delivery),
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

async fn write_runner<Wr, U, C>(
    writer: Wr,
    rx_directive: mpsc::Receiver<DeferredDirective>,
    w_ctx: Weak<RpcCore<U, C>>,
) -> WriteRunnerResult
where
    Wr: AsyncFrameWrite,
    C: Codec,
{
    // Implements bulk receive to minimize number of polls on the channel

    let exec_result = async {
        futures::pin_mut!(writer); // Constrain lifetime of `writer` to this block.

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
                    writer
                        .as_mut()
                        .start_frame()
                        .map_err(WriteRunnerError::WriteFailed)?;

                    poll_fn(|cx| writer.as_mut().poll_write_frame(cx, &mut payload))
                        .await
                        .map_err(WriteRunnerError::WriteFailed)?;
                }
                DeferredDirective::WriteMsgBurst(_) => todo!(),

                DeferredDirective::WriteReqMsg(mut payload, req_id) => {
                    let write_result =
                        poll_fn(|cx| writer.as_mut().poll_write_frame(cx, &mut payload))
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
