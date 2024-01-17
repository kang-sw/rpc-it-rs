use std::{
    borrow::Cow,
    future::Future,
    mem::replace,
    num::NonZeroU64,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Poll, Waker},
};

use bytes::Bytes;
use futures::task::AtomicWaker;
use hashbrown::HashMap;
use parking_lot::Mutex;

use crate::{
    codec::{Codec, ParseMessage, ResponseError},
    defs::RequestId,
};

use super::{error::ErrorResponse, Config, ReceiveResponse};

/// Response message from RPC server.
#[derive(Debug)]
pub struct Response<C> {
    codec: C,
    payload: Bytes,
}

/// A context for pending RPC requests.
#[derive(Debug)]
pub(crate) struct RequestContext<C> {
    /// Codec of owning RPC connection.
    codec: C,

    /// A set of pending requests that are waiting to be responded.        
    wait_list: Mutex<HashMap<RequestId, Arc<PendingTask>>>,

    /// Is the request context still alive? This is to prevent further request allocations.
    expired: AtomicBool,
}

#[derive(Default, Debug)]
struct PendingTask {
    waker: AtomicWaker,
    response: Mutex<ResponseData>,
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

pub(super) struct ReceiveResponseState {
    request_id: RequestId,
}

impl ReceiveResponseState {
    pub(super) fn new(id: RequestId) -> Self {
        todo!()
    }

    pub(super) fn request_id(&self) -> RequestId {
        self.request_id
    }
}

impl<'a, R> Future for ReceiveResponse<'a, R>
where
    R: Config,
{
    type Output = Result<Response<R::Codec>, Option<ErrorResponse<R::Codec>>>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        /*
            TODO: Implement state machine for response receiving.

            - Init: Fetch reference to the slot. Try to retrieve the response from the slot.
            - Pending: Try to retrieve response ...
            - Expired: panic!
        */

        todo!();

        Poll::Pending
    }
}

impl<'a, R: Config> Drop for ReceiveResponse<'a, R> {
    fn drop(&mut self) {
        todo!()
    }
}

// ======== ReceiveResponse ======== //

impl<'a, R: Config> ReceiveResponse<'a, R> {
    /// Elevate the lifetime of the response to `'static`.
    pub fn into_owned(mut self) -> ReceiveResponse<'static, R> {
        ReceiveResponse {
            owner: Cow::Owned((*self.owner).clone()),
            state: self.state.take(),
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.state.as_ref().unwrap().request_id
    }
}

// ========================================================== Response ===|

impl<C: Codec> ParseMessage<C> for Response<C> {
    fn codec_payload_pair(&self) -> (&C, &[u8]) {
        (&self.codec, self.payload.as_ref())
    }
}

impl<C: Codec> ErrorResponse<C> {
    pub fn errc(&self) -> ResponseError {
        self.errc
    }
}

impl<C: Codec> ParseMessage<C> for ErrorResponse<C> {
    fn codec_payload_pair(&self) -> (&C, &[u8]) {
        (&self.codec, self.payload.as_ref())
    }
}

pub(super) fn make_response<C: Codec>(
    codec: C,
    payload: Bytes,
    errc: Option<ResponseError>,
) -> Result<Response<C>, ErrorResponse<C>> {
    if let Some(errc) = errc {
        Err(ErrorResponse {
            codec,
            payload,
            errc,
        })
    } else {
        Ok(Response { codec, payload })
    }
}

// ==== RequestContext ====

impl<C: Codec> RequestContext<C> {
    pub(super) fn new(codec: C) -> Self {
        Self {
            codec,
            wait_list: Default::default(),
            expired: Default::default(),
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
    pub(super) fn try_allocate_new_request(&self, id: RequestId) -> Option<ReceiveResponseState> {
        todo!()
    }

    /// Mark the request ID as free.
    ///
    /// # Panics
    ///
    /// Panics if the request ID is not found from the registry
    pub(super) fn cancel_request_alloc(&self, id: ReceiveResponseState) {
        todo!()
    }

    /// Sets the response for the request ID.
    ///
    /// Called from the background receive runner.
    pub fn set_response(
        &self,
        id: RequestId,
        payload: Bytes,
        errc: Option<ResponseError>,
    ) -> Result<(), Bytes> {
        todo!()
    }

    /// Invalidate all pending requests. This is called when the connection is closed.
    ///
    /// This will be called either after when the deferred runner rx channel is closed.
    pub fn mark_expired(&self) {
        // Don't let the pending tasks to be registered anymore.
        if self.expired.swap(true, Ordering::SeqCst) {
            return; // Already expired
        }

        todo!()
    }

    /// Called by deferred runner, when the request is canceled due to write error
    pub fn invalidate_request(&self, id: RequestId) {
        todo!()
    }
}
