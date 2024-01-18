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
pub(crate) struct RequestContext {
    /// A set of pending requests that are waiting to be responded.        
    wait_list: Mutex<HashMap<RequestId, Arc<TaskWaitObject>>>,

    /// Is the request context still alive? This is to prevent further request allocations.
    expired: AtomicBool,
}

#[derive(Default, Debug)]
struct TaskWaitObject {
    waker: AtomicWaker,
    response: Mutex<ResponseData>,
}

#[derive(Default, Debug)]
enum ResponseData {
    #[default]
    None,
    Ready(Bytes, Option<ResponseError>),
    Closed,
    PendingRemove,
}

// ========================================================== ReceiveResponse ===|

pub(super) struct ReceiveResponseState {
    request_id: RequestId,
    wait_obj: Arc<TaskWaitObject>,
}

impl ReceiveResponseState {
    pub(super) fn new(id: RequestId) -> Self {
        Self {
            request_id: id,
            wait_obj: Default::default(),
        }
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

        let Some(obj) = this.state.as_mut() else {
            // There are two cases for this:
            // - Polled after `Ready`
            // - We already retrieved the value using `try_recv`
            // Both of cases are just logic errors are can be detected in user-side,
            crate::cold_path();
            return Poll::Ready(Err(None));
        };

        // Don't let us miss the waker.
        obj.wait_obj.waker.register(cx.waker());

        // Try to get the response.
        match obj.poll() {
            Poll::Pending => {
                // Check if the context is expired
                // * NOTE: This MUST BE placed after waker registration, prevent `expired` flag is
                //         falsy-false when the task is woken up. See `ReqeustContext::mark_expired`
                if context.is_expired_seq_cst() {
                    this.try_unregister();
                    return Poll::Ready(Err(None));
                }

                Poll::Pending
            }
            Poll::Ready(Some((payload, errc))) => {
                let result = this.convert_result(payload, errc);
                this.try_unregister();
                Poll::Ready(result.map_err(Some))
            }
            Poll::Ready(None) => {
                this.try_unregister();
                Poll::Ready(Err(None))
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
        let context = self.owner.reqs();

        let Some(obj) = self.state.as_mut() else {
            return Err(TryRecvResponseError::Retrieved);
        };

        match obj.poll() {
            Poll::Pending => {
                if context.is_expired() {
                    // There's no possible way to receive the response, as the context is already
                    // expired.
                    return Err(TryRecvResponseError::Closed);
                }

                Err(TryRecvResponseError::Empty)
            }
            Poll::Ready(Some((payload, errc))) => {
                let result = self.convert_result(payload, errc);
                self.try_unregister();
                result.map_err(TryRecvResponseError::Response)
            }
            Poll::Ready(None) => {
                self.try_unregister();
                Err(TryRecvResponseError::Closed)
            }
        }
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

impl ReceiveResponseState {
    fn poll(&self) -> Poll<Option<(Bytes, Option<ResponseError>)>> {
        let mut data_lock = self.wait_obj.response.lock();
        let data_ref = &mut *data_lock;

        match data_ref {
            ResponseData::None => Poll::Pending,
            x @ ResponseData::Ready(_, _) => {
                let ResponseData::Ready(data, errc) = replace(x, ResponseData::PendingRemove)
                else {
                    unreachable!()
                };

                Poll::Ready(Some((data, errc)))
            }
            ResponseData::Closed => Poll::Ready(None),
            ResponseData::PendingRemove => {
                unreachable!("unreachable state")
            }
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

impl RequestContext {
    pub(super) fn new() -> Self {
        Self {
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
        // As it's nearly impossible to collide with the existing request ID, here we preemptively
        // allocate the response wait obejct to insert into the pending task list quickly.
        let wait_obj = Arc::new(TaskWaitObject::default());

        if self.wait_list.lock().try_insert(id, wait_obj).is_ok() {
            Some(ReceiveResponseState::new(id))
        } else {
            None
        }
    }

    /// Mark the request ID as free.
    ///
    /// # Panics
    ///
    /// Panics if the request ID is not found from the registry
    pub(super) fn free_request_id(&self, id: RequestId) {
        // The remove operation must be successful, as the only manipulation of the pending task
        // list is done by the `try_allocate_new_request` method.
        assert!(self.wait_list.lock().remove(&id).is_some());
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
        let Some(wait_obj) = self.wait_list.lock().get(&id).cloned() else {
            return Err(payload);
        };

        if self.is_expired() {
            // Here we check if expired after acquiring wait_list lock;
            return Err(payload);
        }

        {
            let mut data = wait_obj.response.lock();

            // `ResponseData` is changed by:
            // 1) here
            // 2) client cancelation
            // 3) individual invalidation
            //
            // It is allowed the response data is `Ready`, and it's trivial error as usually this
            // means that the remote side sent duplicated response id, that is not our
            // responsibility.
            //
            // Otherwise, if following abort fails, it means:
            // - We're
            #[cfg(debug_assertions)]
            assert!(matches!(
                *data,
                ResponseData::None | ResponseData::Ready(..)
            ));

            *data = ResponseData::Ready(payload, errc);
        }

        // Wakeup the task AFTER registering the response.
        wait_obj.waker.wake();

        Ok(())
    }

    /// Invalidate all pending requests. This is called when the connection is closed.
    ///
    /// This will be called either after when the deferred runner rx channel is closed.
    pub fn mark_expired(&self) {
        // Acquire lock first, to make
        let wait_list = self.wait_list.lock();

        // Don't let the pending tasks to be registered anymore.
        if self.expired.swap(true, Ordering::SeqCst) {
            return; // Already expired
        }

        // Wakeup all awaiting tasks. They must see the `expired` flag set as true and return
        // immediately with error.
        wait_list.iter().for_each(|(.., obj)| {
            obj.waker.wake();
        });
    }

    pub fn is_expired(&self) -> bool {
        self.expired.load(Ordering::Acquire)
    }

    pub fn is_expired_seq_cst(&self) -> bool {
        self.expired.load(Ordering::SeqCst)
    }

    /// Called by deferred runner, when the request is canceled due to write error
    pub fn invalidate_request(&self, id: RequestId) {
        todo!()
    }
}

#[cfg(debug_assertions)]
impl Drop for RequestContext {
    fn drop(&mut self) {
        // As the `ReceiveResponse` instance is holding the reference to `RpcCore` object which
        // contains this `RequestContext` object, it must be guaranteed that all `ReceiveResponse`
        // objects are dropped before this `RequestContext` object is dropped; i.e. all pending
        // waitlist is cleared by drop guard of each `ReceiveResponse`.
        assert!(self.wait_list.lock().is_empty());
    }
}
