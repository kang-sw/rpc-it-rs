use std::{
    borrow::Cow,
    mem::replace,
    num::NonZeroU64,
    sync::{atomic::AtomicU64, Arc},
    task::Poll,
};

use dashmap::DashMap;
use futures_util::{task::AtomicWaker, FutureExt};
use parking_lot::Mutex;

use crate::RecvError;

use super::{msg, Connection};

/// RPC request context. Stores request ID and response receiver context.
#[derive(Debug, Default)]
pub struct RequestContext {
    /// We may not run out of 64-bit sequential integers ...
    req_id_gen: AtomicU64,
    waiters: DashMap<NonZeroU64, RequestSlot>,
}

#[derive(Clone, Debug)]
pub(super) struct RequestSlotId(NonZeroU64);

#[derive(Debug)]
struct RequestSlot {
    waker: AtomicWaker,
    value: Mutex<Option<msg::Response>>,
}

impl RequestContext {
    /// Never returns 0.
    pub(super) fn next_req_id_base(&self) -> NonZeroU64 {
        // SAFETY: 1 + 2 * N is always odd, and non-zero on wrapping condition.
        // > Additionally, to see it wraps, we need to send 2^63 requests ...
        unsafe {
            NonZeroU64::new_unchecked(
                1 + self.req_id_gen.fetch_add(2, std::sync::atomic::Ordering::Relaxed),
            )
        }
    }

    #[must_use]
    pub(super) fn register_req(&self, req_id_hash: NonZeroU64) -> RequestSlotId {
        let slot = RequestSlot { waker: AtomicWaker::new(), value: Mutex::new(None) };
        if self.waiters.insert(req_id_hash, slot).is_some() {
            panic!("Request ID collision")
        }
        RequestSlotId(req_id_hash)
    }

    /// Routes given response to appropriate handler. Returns `Err` if no handler is found.
    pub(super) fn route_response(
        &self,
        req_id_hash: NonZeroU64,
        response: msg::Response,
    ) -> Result<(), msg::Response> {
        let Some(slot) = self.waiters.get(&req_id_hash) else {
            return Err(response);
        };

        let mut value = slot.value.lock();
        value.replace(response);
        slot.waker.wake();

        Ok(())
    }

    /// Wake up all waiters. This is to abort all pending response futures that are waiting on
    /// closed connections. This doesn't do more than that, as the request context itself can
    /// be shared among multiple connections.
    pub(super) fn wake_up_all(&self) {
        for x in self.waiters.iter() {
            x.waker.wake();
        }
    }
}

/// When dropped, the response handler will be unregistered from the queue.
#[must_use = "futures do nothing unless you `.await` or poll them"]
#[derive(Debug)]
pub struct ResponseFuture<'a>(ResponseFutureInner<'a>);

#[must_use = "futures do nothing unless you `.await` or poll them"]
#[derive(Debug)]
pub struct OwnedResponseFuture(ResponseFuture<'static>);

#[derive(Debug)]
enum ResponseFutureInner<'a> {
    Waiting(Cow<'a, Arc<dyn Connection>>, NonZeroU64),
    Finished,
}

impl<'a> ResponseFuture<'a> {
    pub(super) fn new(handle: &'a Arc<dyn Connection>, slot_id: RequestSlotId) -> Self {
        Self(ResponseFutureInner::Waiting(Cow::Borrowed(handle), slot_id.0))
    }

    pub fn to_owned(mut self) -> OwnedResponseFuture {
        let state = replace(&mut self.0, ResponseFutureInner::Finished);

        match state {
            ResponseFutureInner::Waiting(conn, id) => OwnedResponseFuture(ResponseFuture(
                ResponseFutureInner::Waiting(Cow::Owned(conn.into_owned()), id),
            )),
            ResponseFutureInner::Finished => {
                OwnedResponseFuture(ResponseFuture(ResponseFutureInner::Finished))
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<Option<msg::Response>, RecvError> {
        use ResponseFutureInner::*;

        match &mut self.0 {
            Waiting(conn, hash) => {
                if conn.__is_disconnected() {
                    // Let the 'drop' trait erase the request from the queue.
                    return Err(RecvError::Disconnected);
                }

                let mut value = None;
                conn.reqs().unwrap().waiters.remove_if(hash, |_, elem| {
                    if let Some(v) = elem.value.lock().take() {
                        value = Some(v);
                        true
                    } else {
                        false
                    }
                });

                if value.is_some() {
                    self.0 = Finished;
                }

                Ok(value)
            }

            Finished => Ok(None),
        }
    }
}

impl<'a> std::future::Future for ResponseFuture<'a> {
    type Output = Result<msg::Response, RecvError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use ResponseFutureInner::*;

        match &mut self.0 {
            Waiting(conn, hash) => {
                if conn.__is_disconnected() {
                    // Let the 'drop' trait erase the request from the queue.
                    return Poll::Ready(Err(RecvError::Disconnected));
                }

                let mut value = None;
                conn.reqs().unwrap().waiters.remove_if(hash, |_, elem| {
                    if let Some(v) = elem.value.lock().take() {
                        value = Some(v);
                        true
                    } else {
                        elem.waker.register(cx.waker());
                        false
                    }
                });

                if let Some(value) = value {
                    self.0 = Finished;
                    Poll::Ready(Ok(value))
                } else {
                    Poll::Pending
                }
            }

            Finished => panic!("ResponseFuture polled after completion"),
        }
    }
}

impl std::future::Future for OwnedResponseFuture {
    type Output = Result<msg::Response, RecvError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.0.poll_unpin(cx)
    }
}

impl futures_util::future::FusedFuture for ResponseFuture<'_> {
    fn is_terminated(&self) -> bool {
        matches!(self.0, ResponseFutureInner::Finished)
    }
}

impl futures_util::future::FusedFuture for OwnedResponseFuture {
    fn is_terminated(&self) -> bool {
        self.0.is_terminated()
    }
}

impl std::ops::Deref for OwnedResponseFuture {
    type Target = ResponseFuture<'static>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for OwnedResponseFuture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ResponseFuture<'_> {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut self.0, ResponseFutureInner::Finished);
        let ResponseFutureInner::Waiting(conn, hash) = state else { return };
        let reqs = conn.reqs().unwrap();
        assert!(
            reqs.waiters.remove(&hash).is_some(),
            "Request lifespan must be bound to this future."
        );
    }
}
