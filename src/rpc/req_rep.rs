use std::{
    borrow::Cow,
    future::Future,
    mem::replace,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
};

use bytes::Bytes;
use futures::{future::FusedFuture, task::AtomicWaker};
use hashbrown::HashMap;
use parking_lot::Mutex;

use crate::{
    codec::{Codec, ParseMessage, ResponseError},
    defs::RequestId,
};

use super::{error::ErrorResponse, Config, ReceiveResponse, ResponseReceiveError};

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
    wait_list: Mutex<HashMap<RequestId, Arc<Waiter>>>,

    /// Is the request context still alive? This is to prevent further request allocations.
    expired: AtomicBool,
}

#[derive(Default, Debug)]
struct Waiter {
    waker: AtomicWaker,
    response: Mutex<ResponseState>,
}

#[derive(Default, Debug)]
enum ResponseState {
    #[default]
    None,
    Ready(Bytes, Option<ResponseError>),
    WriteFailed,
    PendingRemove,
}

// ========================================================== ReceiveResponse ===|

pub(super) struct Receive {
    request_id: RequestId,
    wait_obj: Arc<Waiter>,
}

impl Receive {
    fn new(id: RequestId, wait_obj: Arc<Waiter>) -> Self {
        Self {
            request_id: id,
            wait_obj,
        }
    }
}

impl<'a, R> Future for ReceiveResponse<'a, R>
where
    R: Config,
{
    type Output = Result<Response<R::Codec>, ResponseReceiveError<R::Codec>>;

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
            return Poll::Ready(Err(ResponseReceiveError::Empty));
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
                    return Poll::Ready(Err(ResponseReceiveError::Closed));
                }

                Poll::Pending
            }
            Poll::Ready(Some((payload, errc))) => {
                let result = this.convert_result(payload, errc);
                this.try_unregister();
                Poll::Ready(result.map_err(ResponseReceiveError::Response))
            }
            Poll::Ready(None) => {
                this.try_unregister();
                Poll::Ready(Err(ResponseReceiveError::Closed))
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

    pub fn try_recv(&mut self) -> Result<Response<R::Codec>, ResponseReceiveError<R::Codec>> {
        let context = self.owner.reqs();

        let Some(obj) = self.state.as_mut() else {
            return Err(ResponseReceiveError::Retrieved);
        };

        match obj.poll() {
            Poll::Pending => {
                if context.is_expired() {
                    // There's no possible way to receive the response, as the context is already
                    // expired.
                    return Err(ResponseReceiveError::Closed);
                }

                Err(ResponseReceiveError::Empty)
            }
            Poll::Ready(Some((payload, errc))) => {
                let result = self.convert_result(payload, errc);
                self.try_unregister();
                result.map_err(ResponseReceiveError::Response)
            }
            Poll::Ready(None) => {
                self.try_unregister();
                Err(ResponseReceiveError::Closed)
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

impl Receive {
    fn poll(&self) -> Poll<Option<(Bytes, Option<ResponseError>)>> {
        let mut data_lock = self.wait_obj.response.lock();
        let data_ref = &mut *data_lock;

        match data_ref {
            ResponseState::None => Poll::Pending,
            x @ ResponseState::Ready(_, _) => {
                let ResponseState::Ready(data, errc) = replace(x, ResponseState::PendingRemove)
                else {
                    unreachable!()
                };

                Poll::Ready(Some((data, errc)))
            }
            ResponseState::WriteFailed => Poll::Ready(None),
            ResponseState::PendingRemove => {
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
    pub(super) fn try_allocate_new_request(&self, id: RequestId) -> Option<Receive> {
        // As it's nearly impossible to collide with the existing request ID, here we preemptively
        // allocate the response wait obejct to insert into the pending task list quickly.
        let wait_obj = Arc::new(Waiter::default());
        let mut wait_list = self.wait_list.lock();

        if self.is_expired() {
            // We can naively check for expiration here, as it is a valid operation to create a new
            // awaiter task after the request context has expired. If this happens, the newly created
            // `ReceiveResponse` object will check the expiration flag and unregister itself
            // during its first `poll` or `try_recv` call.

            return None;
        }

        if wait_list.try_insert(id, wait_obj.clone()).is_ok() {
            Some(Receive::new(id, wait_obj))
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

            // There are two scenarios where we receive a response with `data` not being `None`:
            // - When `data` is `PendingRemove`:
            //    - This is a valid case. It occurs when the user cancels the request, but the
            //      background runner has already received the response before the user removed
            //      the wait object from the wait list.
            // - When `data` is `Ready` or `WriteFailed`:
            //    - This might indicate a logic error from the remote server, as it implies
            //      a response is sent for a request we never made. However, this can also
            //      occur validly if we accidentally use a duplicated request ID (which is
            //      possible since we rely on a random number generator), and the previous
            //      request was cancelled before receiving its response. In such a case,
            //      the server's response is valid for both the server and this client.

            if matches!(*data, ResponseState::None | ResponseState::Ready(..)) {
                // Based on the above reasoning, overwriting the current `Ready` data is
                // likely more correct than retaining it.
                *data = ResponseState::Ready(payload, errc);
            } else {
                // Despite the above reasoning, encountering this case is still quite rare.
                crate::cold_path();
            }
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

        // Don't let the awaiters to be registered anymore.
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
    pub fn set_request_write_failed(&self, id: RequestId) {
        let Some(wait_obj) = self.wait_list.lock().get(&id).cloned() else {
            // This is valid case where the request is already canceled by the user, therefore the
            // entry was removed from the wait list.
            return;
        };

        {
            let mut data = wait_obj.response.lock();

            // Theoretically, here we should check if the current data variant is `None`. However,
            // in practice, there's a possibility that the remote server might send a response that
            // wasn't requested by this client, using a duplicated request ID.
            //
            // Therefore, we choose to naively overwrite the current data variant with the `Closed`
            // variant, regardless of its current state. For the `ReceiveResponse`, it's crucial
            // that the current variant is non-`None` to return `Poll::Ready`.

            *data = ResponseState::WriteFailed;
        }

        // Wakeup the task AFTER modifying the response state.
        wait_obj.waker.wake();
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
