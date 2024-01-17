use core::panic;
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
use futures::{future::FusedFuture, task::AtomicWaker};
use hashbrown::HashMap;
use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    codec::{Codec, ParseMessage, ResponseError},
    defs::RequestId,
};

use super::{error::ErrorResponse, Config, ReceiveResponse, TryRecvResponseError};

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
    Retrieved,
}

// ========================================================== ReceiveResponse ===|

pub(super) struct ReceiveResponseState {
    request_id: RequestId,
    wait_obj: Arc<PendingTask>,
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

        let context = this.owner.reqs();
        if context.expired.load(Ordering::Acquire) {
            return Poll::Ready(Err(None));
        }

        let Some(obj) = this.state.as_mut() else {
            panic!("Polled after response is received")
        };

        // Don't let us miss the waker.
        obj.wait_obj.waker.register(cx.waker());

        // Try to get the response.
        let mut data_lock = obj.wait_obj.response.lock();
        let data_ref = &mut *data_lock;

        match data_ref {
            ResponseData::None => Poll::Pending,
            x @ ResponseData::Ready(_, _) => {
                let ResponseData::Ready(data, errc) = replace(x, ResponseData::Retrieved) else {
                    unreachable!()
                };

                drop(data_lock);
                this.try_unregister();

                Poll::Ready(this.convert_result(data, errc).map_err(Some))
            }
            ResponseData::Closed => {
                drop(data_lock);
                this.try_unregister();

                Poll::Ready(Err(None))
            }
            ResponseData::Retrieved => {
                panic!("Polled after response is received")
            }
        }
    }
}

impl<R: Config> FusedFuture for ReceiveResponse<'_, R> {
    fn is_terminated(&self) -> bool {
        self.state.is_none()
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

    pub fn try_recv(&mut self) -> Result<Response<R::Codec>, TryRecvResponseError<R::Codec>> {
        todo!()
    }

    fn convert_result(
        &self,
        payload: Bytes,
        errc: Option<ResponseError>,
    ) -> Result<Response<R::Codec>, ErrorResponse<R::Codec>> {
        let codec = self.owner.codec().clone();

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

    fn try_unregister(&mut self) {
        if let Some(obj) = self.state.take() {
            // prevent redundant wakeups
            obj.wait_obj.waker.take();

            // unregister
            let context = self.owner.reqs();
            context.free_request_id(obj.request_id);
        }
    }
}

impl<R: Config> Drop for ReceiveResponse<'_, R> {
    fn drop(&mut self) {
        self.try_unregister()
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
    pub(super) fn free_request_id(&self, id: RequestId) {
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
