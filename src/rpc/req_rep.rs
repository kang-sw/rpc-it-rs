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
use serde::de::Deserialize;

use crate::{
    codec::{Codec, ResponseErrorCode},
    defs::RequestId,
};

use super::{
    error::{ErrorResponse, ReceiveResponseError},
    ReceiveResponse,
};

/// Response message from RPC server.
pub struct Response(Arc<dyn Codec>, Bytes);

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

#[derive(Debug)]
struct PendingTask {
    registered_waker: Option<Waker>,
    response: ResponseData,
}

#[derive(Debug)]
enum ResponseData {
    NotReady,
    Ready(Bytes, Option<ResponseErrorCode>),
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

impl<'a> Future for ReceiveResponse<'a> {
    type Output = Result<Response, ReceiveResponseError>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        /*
            TODO: Implement state machine for response receiving.

            - Init: Fetch reference to the slot. Try to retrieve the response from the slot.
            - Pending: Try to retrieve response ...
            - Expired: panic!
        */

        let lc_entry = this.reqs.pending_tasks.read();
        let mut lc_slot = lc_entry
            .get(&this.req_id)
            .expect("'req_id' not found; seems polled after ready!")
            .lock();

        // Check if we're ready to retrieve response
        match &mut lc_slot.response {
            ResponseData::NotReady => (),
            ResponseData::Unreachable => unreachable!("Polled after ready"),
            ResponseData::Closed => return Poll::Ready(Err(ReceiveResponseError::ServerClosed)),
            resp @ ResponseData::Ready(..) => {
                let ResponseData::Ready(buf, errc) = replace(resp, ResponseData::Unreachable)
                else {
                    unreachable!()
                };

                // Drops lock a bit early.
                drop(lc_slot);
                drop(lc_entry);

                // NOTE: In this line, we're 'trying' to upgrade the pointer and checks if it's
                // expired or not. However, in actual implementation, the backed `RpcContext`
                // implementor will be cast itself to `Arc<dyn Codec>` and will be cloned, which
                // means, as long as the `RequestContext` is alive, the `Arc<dyn Codec>` will
                // never be invalidated.
                let codec = this
                    .reqs
                    .codec
                    .upgrade()
                    .ok_or(ReceiveResponseError::ClientClosed)?;

                return Poll::Ready(if let Some(errc) = errc {
                    Err(ReceiveResponseError::ErrorResponse(ErrorResponse {
                        errc,
                        codec,
                        buf,
                    }))
                } else {
                    Ok(Response(codec, buf))
                });
            }
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

impl<'a> Drop for ReceiveResponse<'a> {
    fn drop(&mut self) {
        if matches!(self.state, ReceiveResponseState::Expired) {
            return;
        }

        let _e = self.reqs.pending_tasks.write().remove(&self.req_id);
        debug_assert!(
            _e.is_some(),
            "ReceiveResponse is dropped before polled to ready!"
        );
    }
}

// ======== ReceiveResponse ======== //

impl<'a> ReceiveResponse<'a> {
    /// Elevate the lifetime of the response to `'static`.
    pub fn into_owned(mut self) -> ReceiveResponse<'static> {
        ReceiveResponse {
            reqs: Cow::Owned((*self.reqs).clone()),
            req_id: self.req_id,
            state: replace(&mut self.state, ReceiveResponseState::Expired),
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.req_id
    }
}

// ========================================================== Response ===|

impl Response {
    /// Try parse the response as the given type.
    pub fn parse<R>(&self) -> erased_serde::Result<R>
    where
        R: for<'de> Deserialize<'de>,
    {
        todo!()
    }
}

impl ErrorResponse {
    // TODO: Get Code, Parse, ...
}

// ==== RequestContext ====

impl RequestContext {
    pub(super) fn allocate_new_request(&self) -> RequestId {
        todo!()
    }

    /// Mark the request ID as free.
    ///
    /// # Panics
    ///
    /// Panics if the request ID is not found from the registry
    pub(super) fn free_id(&self, id: RequestId) {
        todo!()
    }

    /// Set the response for the request ID.
    pub(crate) fn set_response(&self, id: RequestId, resp: Response) {
        todo!()
    }
}
