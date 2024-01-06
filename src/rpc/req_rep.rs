use std::{
    borrow::Cow,
    future::Future,
    mem::replace,
    sync::{atomic::AtomicU32, Arc, Weak},
    task::{Poll, Waker},
};

use bytes::Bytes;
use hashbrown::HashMap;
use parking_lot::{Mutex, RwLock};

use crate::{
    codec::{Codec, ParseMessage, ResponseError},
    defs::RequestId,
};

use super::{
    error::{ErrorResponse, ReceiveResponseError},
    ReceiveResponse,
};

/// Response message from RPC server.
pub struct Response {
    codec: Arc<dyn Codec>,
    payload: Bytes,
}

/// A context for pending RPC requests.
#[derive(Debug)]
pub(super) struct RequestContext {
    /// Codec of owning RPC connection.
    codec: Weak<dyn Codec>,

    /// Request ID generator. Rotates every 2^32 requests.
    ///
    /// It naively expects that the request ID is not reused until 2^32 requests are made.
    req_id_gen: AtomicU32,

    /// A set of pending requests that are waiting to be responded.        
    pending_tasks: RwLock<HashMap<RequestId, Mutex<PendingTask>>>,
}

#[derive(Default, Debug)]
struct PendingTask {
    registered_waker: Option<Waker>,
    response: ResponseData,
}

#[derive(Default, Debug)]
enum ResponseData {
    #[default]
    None,
    Ready(Bytes, Option<ResponseError>),
    Closed,
    Unreachable,
}

// ========================================================== ReceiveResponse ===|

#[derive(Default)]
pub(super) enum ReceiveResponseState {
    /// A waker that is registered to the pending task.
    #[default]
    Init,
    Pending(Waker),

    /// Required to implement [`ReceiveResponse::into_owned`]
    Expired,
}

impl<'a, U> Future for ReceiveResponse<'a, U> {
    type Output = Result<Response, ReceiveResponseError>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        /*
            TODO: Implement state machine for response receiving.

            - Init: Fetch reference to the slot. Try to retrieve the response from the slot.
            - Pending: Try to retrieve response ...
            - Expired: panic!
        */

        let lc_entry = this.owner.req.pending_tasks.read();
        let mut lc_slot = lc_entry
            .get(&this.req_id)
            .expect("'req_id' not found; seems polled after ready!")
            .lock();

        // Check if we're ready to retrieve response
        match &mut lc_slot.response {
            ResponseData::None => (),
            ResponseData::Closed => return Poll::Ready(Err(ReceiveResponseError::Disconnected)),
            resp @ ResponseData::Ready(..) => {
                let resp = replace(resp, ResponseData::Unreachable);
                let ResponseData::Ready(payload, errc) = resp else {
                    unreachable!()
                };

                // Drops lock a bit early.
                drop(lc_slot);
                drop(lc_entry);

                // NOTE: In this line, we're 'trying' to upgrade the pointer and checks if it's
                // expired or not. However, in actual implementation, the backed `RpcContext`
                // implementor will be cast itself to `Arc<dyn Codec>` and will be cloned, which
                // means, as long as the `RequestContext` is alive, the `Arc<dyn Codec>` will never
                // be invalidated.
                //
                // However, here, we explicitly check if the `Arc<dyn Codec>` is still alive or not
                // to future change of internal implementation.
                let codec = this
                    .owner
                    .req
                    .codec
                    .upgrade()
                    .ok_or(ReceiveResponseError::Disconnected)?;

                return Poll::Ready(if let Some(errc) = errc {
                    Err(ReceiveResponseError::ErrorResponse(ErrorResponse {
                        errc,
                        codec,
                        payload,
                    }))
                } else {
                    Ok(Response { codec, payload })
                });
            }

            ResponseData::Unreachable => panic!("Polled after ready"),
        }

        let new_waker = match &mut this.state {
            ReceiveResponseState::Init => {
                let waker = cx.waker().clone();
                this.state = ReceiveResponseState::Pending(waker.clone());
                Some(waker)
            }
            ReceiveResponseState::Pending(waker) => {
                let new_waker = cx.waker();
                if waker.will_wake(new_waker) {
                    None // We can reuse the waker (optimize it away)
                } else {
                    *waker = new_waker.clone();
                    Some(waker.clone())
                }
            }
            ReceiveResponseState::Expired => panic!("Polled after ready"),
        };

        if let Some(wk) = new_waker {
            lc_slot.registered_waker = Some(wk);
        }

        Poll::Pending
    }
}

impl<'a, U> Drop for ReceiveResponse<'a, U> {
    fn drop(&mut self) {
        if matches!(self.state, ReceiveResponseState::Expired) {
            return;
        }

        let _e = self.owner.req.pending_tasks.write().remove(&self.req_id);
        debug_assert!(
            _e.is_some(),
            "ReceiveResponse is dropped before polled to ready!"
        );
    }
}

// ======== ReceiveResponse ======== //

impl<'a, U> ReceiveResponse<'a, U> {
    /// Elevate the lifetime of the response to `'static`.
    pub fn into_owned(mut self) -> ReceiveResponse<'static, U> {
        ReceiveResponse {
            owner: Cow::Owned((*self.owner).clone()),
            req_id: self.req_id,
            state: replace(&mut self.state, ReceiveResponseState::Expired),
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.req_id
    }
}

// ========================================================== Response ===|

impl ParseMessage for Response {
    fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]) {
        (self.codec.as_ref(), self.payload.as_ref())
    }
}

impl ErrorResponse {
    pub fn errc(&self) -> ResponseError {
        self.errc
    }
}

impl ParseMessage for ErrorResponse {
    fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]) {
        (self.codec.as_ref(), self.payload.as_ref())
    }
}

// ==== RequestContext ====

impl RequestContext {
    pub(super) fn new(codec: Weak<dyn Codec>) -> Self {
        Self {
            codec,
            req_id_gen: Default::default(),
            pending_tasks: Default::default(),
        }
    }

    /// Allocate a new request ID.
    ///
    /// Returns a newly allocated nonzero request ID. This function is used to generate unique
    /// identifiers for requests. It increments the request ID generator and checks for duplicates
    /// in the pending tasks table. If a duplicate is found, it continues generating IDs until a
    /// unique one is found.
    ///
    /// # Returns
    ///
    /// The newly allocated request ID.
    pub(super) fn allocate_new_request(&self) -> RequestId {
        let mut table = self.pending_tasks.write();
        loop {
            let Ok(id) = self
                .req_id_gen
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .wrapping_add(1)
                .try_into()
            else {
                // It should be noted that this branch is rarely used; the rotation of IDs is not
                // expected to occur frequently.

                crate::cold_path();
                continue;
            };

            let id = RequestId::new(id);
            let mut duplicated = false;
            table
                .entry(id)
                .and_modify(|_| duplicated = true)
                .or_insert(Default::default());

            if duplicated {
                // In a context similar to the one mentioned above, to ensure compliance with
                // duplication policies, the following conditions must be met:
                // - The request ID has undergone rotation (after 2^32 requests have been made)
                // - The request ID has not yet been released. (This involves waiting until 2^32
                //   requests have been made. In any case, this situation would most likely indicate
                //   a bug or a resource leak.)

                crate::cold_path();
                continue;
            }

            break id;
        }
    }

    /// Mark the request ID as free.
    ///
    /// # Panics
    ///
    /// Panics if the request ID is not found from the registry
    pub(super) fn cancel_request_alloc(&self, id: RequestId) {
        let mut table = self.pending_tasks.write();

        let _e = table.remove(&id);
        debug_assert!(_e.is_some(), "Request ID not found from the registry!");
    }

    /// Sets the response for the request ID.
    ///
    /// Called from the background receive runner.
    pub(crate) fn set_response(&self, id: RequestId, data: Bytes, errc: Option<ResponseError>) {
        let table = self.pending_tasks.read();
        let Some(mut slot) = table.get(&id).map(|x| x.lock()) else {
            // User canceled the request before the response was received.
            return;
        };

        slot.set_then_wake(ResponseData::Ready(data, errc));
    }

    /// Invalidate all pending requests. This is called when the connection is closed.
    ///
    /// This will be called after deferred runner rx channel is closed.
    pub(crate) fn invalidate_all_requests(&self) {
        let table = self.pending_tasks.read();
        for (_, slot) in table.iter() {
            slot.lock().set_then_wake(ResponseData::Closed)
        }
    }

    /// Called by deferred runner, when the request is canceled due to write error
    pub(crate) fn invalidate_request(&self, id: RequestId) {
        let table = self.pending_tasks.read();
        let Some(mut slot) = table.get(&id).map(|x| x.lock()) else {
            // User canceled the request before the response was received.
            return;
        };

        slot.set_then_wake(ResponseData::Closed)
    }
}

impl PendingTask {
    fn set_then_wake(&mut self, resp: ResponseData) {
        self.response = resp;

        if let Some(waker) = self.registered_waker.take() {
            waker.wake();
        }
    }
}
