//! Internal driver context. It receives the request from the connection, and dispatches it to
//! the handler.

use bytes::Bytes;
use capture_it::capture;
use futures_util::{future::FusedFuture, FutureExt};
use std::{future::poll_fn, num::NonZeroU64, sync::Arc};

use crate::{
    codec::{self, Codec, InboundFrameType},
    rpc::{DeferredWrite, SendError},
    transport::{AsyncFrameRead, AsyncFrameWrite, FrameReader},
};

use super::{
    msg, ConnectionImpl, DriverBody, Feature, GetRequestContext, InboundBody,
    InboundDriverDirective, InboundError, InboundEventSubscriber, MessageReqId, UserData,
};

/// Request-acceting version of connection driver
impl<C, T, R, U> ConnectionImpl<C, T, R, U>
where
    C: Codec,
    T: AsyncFrameWrite,
    R: GetRequestContext,
    U: UserData,
{
    pub(crate) async fn inbound_event_handler<Tr, E>(body: DriverBody<C, T, E, R, U, Tr>)
    where
        Tr: AsyncFrameRead,
        E: InboundEventSubscriber,
    {
        body.execute().await;
    }
}

impl<C, T, E, R, U, Tr> DriverBody<C, T, E, R, U, Tr>
where
    C: Codec,
    T: AsyncFrameWrite,
    E: InboundEventSubscriber,
    R: GetRequestContext,
    U: UserData,
    Tr: AsyncFrameRead,
{
    async fn execute(self) {
        let DriverBody { w_this, mut read, mut ev_subs, rx_drive: rx_msg, tx_msg } = self;

        use futures_util::future::Fuse;
        let mut fut_drive_msg = Fuse::terminated();
        let mut fut_read = Fuse::terminated();
        let mut close_from_remote = false;

        /* ----------------------------- Background Sender Task ----------------------------- */
        // Tasks that perform deferred write operations in the background. It handles messages
        // sent from non-async non-blocking context, such as 'unhandled' response pushed inside
        // `Drop` handler.
        //
        // XXX: should we split background and directive channel capacities?
        let (tx_bg_sender, rx_bg_sender) =
            rx_msg.capacity().map(flume::bounded).unwrap_or_else(flume::unbounded);

        let fut_bg_sender = capture!([w_this], async move {
            let mut pool = super::WriteBuffer::default();
            while let Ok(msg) = rx_bg_sender.recv_async().await {
                let Some(this) = w_this.upgrade() else { break };
                match msg {
                    DeferredWrite::ErrorResponse(req, err) => {
                        let err = this.dyn_ref().__send_err_predef(&mut pool, &req, &err).await;

                        // Ignore all other error types, as this message is triggered
                        // crate-internally on very limited situations
                        if let Err(SendError::IoError(e)) = err {
                            return Err(e);
                        }
                    }
                    DeferredWrite::Raw(mut msg) => {
                        this.dyn_ref().__write_bytes(&mut msg).await?;
                    }
                    DeferredWrite::Flush => {
                        this.dyn_ref().__flush().await?;
                    }
                }
            }

            Ok::<(), std::io::Error>(())
        });
        let fut_bg_sender = fut_bg_sender.fuse();
        let mut fut_bg_sender = std::pin::pin!(fut_bg_sender);
        let mut read = std::pin::pin!(read);

        /* ------------------------------------ App Loop ------------------------------------ */
        loop {
            if fut_drive_msg.is_terminated() {
                fut_drive_msg = rx_msg.recv_async().fuse();
            }

            if fut_read.is_terminated() {
                fut_read = poll_fn(|cx| read.as_mut().poll_next(cx)).fuse();
            }

            futures_util::select! {
                msg = fut_drive_msg => {
                    match msg {
                        Ok(InboundDriverDirective::DeferredWrite(msg)) => {
                            // This may not fail.
                            tx_bg_sender.send_async(msg).await.ok();
                        }

                        Err(_) | Ok(InboundDriverDirective::Close) => {
                            // Connection disposed by 'us' (by dropping RPC handle). i.e.
                            // `close_from_remote = false`
                            break;
                        }
                    }
                }

                inbound = fut_read => {
                    let Some(this) = w_this.upgrade() else { break };

                    match inbound {
                        Ok(bytes) => {
                            Self::on_read(
                                &this,
                                &mut ev_subs,
                                bytes,
                                &tx_msg
                            )
                            .await;
                        }
                        Err(e) => {
                            close_from_remote = true;
                            ev_subs.on_close(false, Err(e));

                            break;
                        }
                    }
                }

                result = fut_bg_sender => {
                    if let Err(err) = result {
                        close_from_remote = true;
                        ev_subs.on_close(true, Err(err));
                    }

                    break;
                }
            }
        }

        // If we're exitting with alive handle, manually close the write stream.
        if let Some(x) = w_this.upgrade() {
            x.dyn_ref().__close().await.ok();
        }

        // Just try to close the channel
        if !close_from_remote {
            ev_subs.on_close(true, Ok(())); // We're closing this
        }

        // Let all pending requests to be cancelled.
        'cancel: {
            let Some(this) = w_this.upgrade() else { break 'cancel };
            let Some(reqs) = this.reqs.get_req_con() else { break 'cancel };

            // Let the handle recognized as 'disconnected'
            drop(fut_drive_msg);
            drop(rx_msg);

            // Wake up all pending responses
            reqs.wake_up_all();
        }
    }

    async fn on_read(
        this: &Arc<ConnectionImpl<C, T, R, U>>,
        ev_subs: &mut E,
        frame: bytes::Bytes,
        tx_msg: &flume::Sender<msg::RecvMsg>,
    ) {
        let parsed = this.codec.decode_inbound(&frame);
        let (header, payload_span) = match parsed {
            Ok(x) => x,
            Err(e) => {
                ev_subs.on_inbound_error(InboundError::InboundDecodeError(e, frame));
                return;
            }
        };

        let h = InboundBody { buffer: frame, payload: payload_span, codec: this.codec.clone() };
        match header {
            InboundFrameType::Notify { .. } | InboundFrameType::Request { .. } => {
                let (msg, disabled) = match header {
                    InboundFrameType::Notify { method } => (
                        msg::RecvMsg::Notify(msg::Notify { h, method, sender: this.clone() }),
                        this.features.contains(Feature::NO_RECEIVE_NOTIFY),
                    ),
                    InboundFrameType::Request { method, req_id } => (
                        msg::RecvMsg::Request(msg::Request {
                            body: Some((msg::RequestInner { h, method, req_id }, this.clone())),
                        }),
                        this.features.contains(Feature::NO_RECEIVE_REQUEST),
                    ),
                    _ => unreachable!(),
                };

                if disabled {
                    ev_subs.on_inbound_error(InboundError::DisabledInbound(msg));
                } else {
                    tx_msg.send_async(msg).await.ok();
                }
            }

            InboundFrameType::Response { req_id, req_id_hash, is_error } => {
                let response = msg::Response { h, is_error, req_id };

                let Some(reqs) = this.reqs.get_req_con() else {
                    ev_subs.on_inbound_error(InboundError::RedundantResponse(response));
                    return;
                };

                let Some(req_id_hash) = NonZeroU64::new(req_id_hash) else {
                    ev_subs.on_inbound_error(InboundError::ResponseHashZero(response));
                    return;
                };

                if let Err(msg) = reqs.route_response(req_id_hash, response) {
                    ev_subs.on_inbound_error(InboundError::ExpiredResponse(msg));
                }
            }
        }
    }
}

/// SAFETY: `ConnectionImpl` is `!Unpin`, thus it is safe to pin the reference.
macro_rules! pin {
    ($this:ident, $ident:ident) => {
        let mut $ident = $this.write().lock().await;
        let mut $ident = unsafe { std::pin::Pin::new_unchecked(&mut *$ident) };
    };
}

impl dyn super::Connection {
    pub(crate) async fn __send_err_predef(
        &self,
        buf: &mut super::WriteBuffer,
        recv: &msg::RequestInner,
        error: &codec::PredefinedResponseError,
    ) -> Result<(), super::SendError> {
        buf.prepare();

        self.codec().encode_response_predefined(recv.req_id(), error, &mut buf.value)?;
        self.__write_buffer(&mut buf.value).await?;
        Ok(())
    }

    /// Write a single frame to the underlying transport. If the transport does not consume the
    /// provided bytes (i.e., it is a read-only transport), the buffer is returned back to the
    /// caller, eliminating the need for reallocation.
    pub(crate) async fn __write_buffer(&self, buf: &mut Vec<u8>) -> std::io::Result<()> {
        #[cfg(debug_assertions)]
        let ptr_orig = buf.as_ptr() as usize;

        let mut bytes = Bytes::from(std::mem::take(buf));
        let original_len = bytes.len();

        self.__write_bytes(&mut bytes).await?;

        if bytes.len() == original_len {
            *buf = Vec::<u8>::from(bytes);

            // Ensure that the buffer is not reallocated
            #[cfg(debug_assertions)]
            debug_assert_eq!(ptr_orig, buf.as_ptr() as usize);
        }

        Ok(())
    }

    /// Write a single frame to the underlying transport. The buffer reference provided may be
    /// completely consumed or remain unmodified.
    pub(crate) async fn __write_bytes(&self, buf: &mut Bytes) -> std::io::Result<()> {
        pin!(self, write);

        write.as_mut().begin_write_frame(buf.len())?;
        let mut reader = FrameReader::new(buf);

        while !reader.is_empty() {
            poll_fn(|cx| write.as_mut().poll_write(cx, &mut reader)).await?;
        }

        Ok(())
    }

    pub(crate) async fn __flush(&self) -> std::io::Result<()> {
        pin!(self, write);
        poll_fn(|cx| write.as_mut().poll_flush(cx)).await
    }

    pub(crate) async fn __close(&self) -> std::io::Result<()> {
        pin!(self, write);
        poll_fn(|cx| write.as_mut().poll_close(cx)).await
    }

    pub(crate) fn __is_disconnected(&self) -> bool {
        self.tx_drive().is_disconnected()
    }
}
