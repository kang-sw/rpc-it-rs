use self::driver::ReqSlot;
use futures::AsyncWriteExt;
use std::sync::{Arc, Weak};

/* ------------------------------------------------------------------------------------------ */
/*                                          RPC TYPES                                         */
/* ------------------------------------------------------------------------------------------ */
#[derive(Debug)]
pub enum Inbound {
    Request(driver::Request),
    Notify(driver::Notify),
}

/* ------------------------------------- Reply Handler Type ------------------------------------- */
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReplyWait {
    slot: Option<Arc<ReqSlot>>,
}

impl Drop for ReplyWait {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            // If we'd polled the future, it would have been taken already.
            //
            // This routine is only called when the future is dropped without being polled
            // or when the future is dropped(aborted) before getting any reply.
            slot.cancel();
        }
    }
}

impl std::future::Future for ReplyWait {
    type Output = Option<driver::Reply>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        driver::ReqSlot::poll_reply(&mut self.slot, cx)
    }
}

/* ------------------------------------------------------------------------------------------ */
/*                                         RPC HANDLE                                         */
/* ------------------------------------------------------------------------------------------ */
///
/// Handle to control RPC driver instance.
///
#[derive(Clone)]
pub struct Handle {
    body: Arc<driver::Instance>,
}

#[derive(thiserror::Error, Debug)]
pub enum TryRecvError {
    #[error("channel is closed.")]
    Closed,

    #[error("no inbound available.")]
    Empty,
}

impl Handle {
    pub fn downgrade(this: &Self) -> WeakHandle {
        WeakHandle {
            w_driver: Arc::downgrade(&this.body),
        }
    }

    /// Check if this RPC channel was closed.
    pub fn is_closed(&self) -> bool {
        self.body.rx_inbound.is_disconnected()
    }

    /// Try to receive single inbound from the remote end.
    pub fn try_recv_inbound(&self) -> Result<Inbound, TryRecvError> {
        self.body.rx_inbound.try_recv().map_err(|e| match e {
            flume::TryRecvError::Empty => TryRecvError::Empty,
            flume::TryRecvError::Disconnected => TryRecvError::Closed,
        })
    }

    /// Receive single inbound from the remote end asynchronously.
    ///
    /// Will return `None` if the channel is closed.
    pub async fn recv_inbound(&self) -> Option<Inbound> {
        self.body.rx_inbound.recv_async().await.ok()
    }

    /// Receive single inbound from the remote end synchronously.
    ///
    /// Will block until an inbound is available, or the channel is closed.
    pub async fn blocking_recv_inbound(&self) -> Option<Inbound> {
        self.body.rx_inbound.recv().ok()
    }

    /// Send a request to the remote end, and returns awaitable reply.
    ///
    /// You can emulate timeout behavior by dropping returned future.
    pub async fn request(
        &self,
        route: &str,
        payload: impl IntoIterator,
    ) -> std::io::Result<ReplyWait> {
        todo!()
    }

    /// Send a notify to the remote end. This will return after writing all payload to underlying
    /// transport.
    pub async fn notify(&self, route: &str, payload: impl IntoIterator) -> std::io::Result<()> {
        let mut body = self.body.write.lock().await;
        let body = &mut *body.as_mut();

        todo!()
    }
}

/* ----------------------------------------- Weak Handle ---------------------------------------- */
#[derive(Clone)]
pub struct WeakHandle {
    w_driver: Weak<driver::Instance>,
}

impl WeakHandle {
    pub fn upgrade(&self) -> Option<Handle> {
        self.w_driver
            .upgrade()
            .map(|driver| Handle { body: driver })
    }
}

/* ------------------------------------------------------------------------------------------ */
/*                                         RPC DRIVER                                         */
/* ------------------------------------------------------------------------------------------ */
pub(crate) mod driver {
    use std::{
        collections::VecDeque,
        ffi::CStr,
        mem::replace,
        pin::Pin,
        ptr::null,
        sync::{
            atomic::{
                AtomicBool,
                Ordering::{AcqRel, Relaxed},
            },
            Arc, Weak,
        },
        task::Poll,
    };

    use dashmap::DashMap;
    use derive_new::new;
    use futures::AsyncReadExt;
    use futures::{AsyncRead, AsyncWrite};
    use parking_lot::Mutex;

    use crate::{
        alias::{default, AsyncMutex, PoolPtr},
        raw,
    };

    /* ------------------------------------------------------------------------------------------ */
    /*                                            TYPES                                           */
    /* ------------------------------------------------------------------------------------------ */

    /// Commonly used reused payload type alias.
    pub type BufferPtr = PoolPtr<Vec<u8>>;

    /// Driver exit result type
    #[derive(thiserror::Error, Debug)]
    pub enum Error {
        #[error("io error: {0}")]
        IoError(#[from] std::io::Error),

        #[error("Invalid identifier {actual:?}, expected {expected:?}")]
        InvalidIdent { expected: [u8; 4], actual: [u8; 4] },
    }

    /* ----------------------------------------- Builder ---------------------------------------- */

    /// Driver initialization information. All RPC session must be initialized with this struct.
    #[derive(typed_builder::TypedBuilder)]
    pub struct InitInfo {
        /// Write interface to the underlying transport layer
        write: Box<dyn AsyncWrite + Send + Sync + Unpin>,

        /// Read interface to the underlying transport layer
        read: Box<dyn AsyncRead + Send + Sync + Unpin>,

        /// Number of maximum buffered notifies/requests that can be queued in buffer.
        /// If this limit is reached, the driver will discard further requests from client.
        ///
        /// Specifying zero here(default) will make the notify buffer unbounded.
        #[builder(default = 0)]
        inbound_channel_size: usize,
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                         DRIVER LOOP                                        */
    /* ------------------------------------------------------------------------------------------ */
    impl InitInfo {
        pub fn start(
            self,
        ) -> (
            super::Handle,
            impl std::future::Future<Output = Result<(), Error>>,
        ) {
            let (tx_inbound, rx_inbound) = flume::bounded(self.inbound_channel_size);
            let (tx_aborts, rx_aborts) = flume::unbounded();

            let reqs = Arc::new(ReqTable::new());

            let d = Instance {
                rx_inbound,
                tx_aborts,
                write: AsyncMutex::new(Box::into_pin(self.write)),
                reqs: reqs.clone(),
            };

            let h = super::Handle { body: Arc::new(d) };
            let weak = Arc::downgrade(&h.body);
            let read = Box::into_pin(self.read);

            (h, run_driver(weak, tx_inbound, rx_aborts, read, reqs))
        }
    }

    /// Receive data from the remote end. For the `notify` and `request` types, create a
    /// receive object, push it to the channel, and route the `reply` type to the entity
    /// waiting for a response as appropriate.
    ///
    /// Sending cancellation handling for cancelled request handlers issued by the `Drop`
    /// routine is also performed by the driver.
    async fn run_driver(
        weak_body: Weak<Instance>,
        tx_inbound: flume::Sender<super::Inbound>,
        rx_aborts: flume::Receiver<u128>,
        mut read: Pin<Box<dyn AsyncRead + Send + Sync + Unpin>>,
        req_table: Arc<ReqTable>,
    ) -> Result<(), Error> {
        // TODO: Payload allocators for various sized request ... (small < 256, medium < 16k, large)

        let read = &mut *read.as_mut();

        todo!()
    }

    /* -------------------------------- Request - Reply Handling -------------------------------- */
    /// Reusable instance of RPC slot.
    pub(crate) struct ReqSlot {
        table: Weak<ReqTable>,
        id: u128,

        drop_flag: AtomicBool,
        data: Mutex<(Option<std::task::Waker>, Option<Reply>)>,
    }

    pub struct Reply {
        head: raw::HRep,
        data: BufferPtr,
    }

    impl ReqSlot {
        pub fn new(table: Weak<ReqTable>, id: u128) -> Self {
            Self {
                table,
                id,
                data: default(),
                drop_flag: false.into(),
            }
        }

        pub fn cancel(self: Arc<ReqSlot>) {
            let handled = self.drop_flag.swap(true, AcqRel);
            let Some(table) = self.table.upgrade() else { return };

            if let Some((_, _)) = table.active.remove(&self.id) {
                {
                    let mut data = self.data.lock();
                    debug_assert!(data.1.is_none());

                    // This is to clear out the waker, if poll is already called more than once,
                    // and we're being aborted.
                    *data = (None, None);
                }

                // This is when the value is not set, and the `cancel` routine must
                // check in the reference.
                table.pool.lock().1.push_back(self);
            } else if handled == true {
                // The value was set already, thus we have to cleanup the value, and reclaim
                // this entity to the queue back.
                *self.data.lock() = (None, None);
                table.pool.lock().1.push_back(self);
            } else {
                // We're dealing with corner cases. Because `try_set_reply` has been called,
                // this entity has already been removed from the `active` table, but `drop_flag`
                // appears to be `false` because `set_reply` is still being called.
                //
                // Since we have raised the `drop_flag` in this routine, the cleanup routine
                // for cancelled replies will be handled by the rest of the logic in the
                // `try_set_reply` routine.
            }
        }

        pub fn poll_reply(
            io_this: &mut Option<Arc<ReqSlot>>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Option<Reply>> {
            let this = io_this.as_mut().unwrap();
            let mut data = this.data.lock();

            if let Some(result) = data.1.take() {
                drop(data);

                let this = io_this.take().unwrap(); // Let it drop.
                if let Some(table) = this.table.upgrade() {
                    // At this point, set_reply has already been called, so this entity cannot
                    // exist in the `active` map of the owner table.
                    debug_assert!(table.active.contains_key(&this.id) == false);

                    // Reclaim the slot.
                    table.pool.lock().1.push_back(this);
                }

                Poll::Ready(Some(result))
            } else if let Some(table) = this.table.upgrade() {
                data.0.replace(cx.waker().clone());
                Poll::Pending
            } else {
                // The table has been dropped, so we can't wait for the reply anymore.
                (drop(data), io_this.take());
                Poll::Ready(None)
            }
        }

        fn set_reply(self: &ReqSlot, result: Reply) {
            let mut data = self.data.lock();
            assert!(data.1.replace(result).is_none());

            if let Some(waker) = data.0.take() {
                waker.wake();
            }
        }
    }

    /* -------------------------------------- Request Table ------------------------------------- */
    #[derive(new)]
    pub(crate) struct ReqTable {
        #[new(default)]
        active: DashMap<u128, Arc<ReqSlot>>,

        /// (id_gen, idle_slots)
        #[new(default)]
        pool: Mutex<(u128, VecDeque<Arc<ReqSlot>>)>,
    }

    impl Drop for ReqTable {
        fn drop(&mut self) {
            //! This logic prevents the polling logic in [`ReqSlot`] from being permanently blocked.
            let all_pending = std::mem::take(&mut self.active);
            all_pending.into_iter().for_each(|(_, slot)| {
                slot.data.lock().0.take().map(|x| x.wake());
            });
        }
    }

    impl ReqTable {
        pub fn alloc(self: &Arc<ReqTable>) -> Arc<ReqSlot> {
            let slot = {
                let (id, slot) = {
                    let mut slots = self.pool.lock();

                    slots.0 += 1;
                    let id = slots.0;

                    (id, slots.1.pop_front())
                };

                if let Some(mut slot) = slot {
                    if let Some(p_slot) = Arc::get_mut(&mut slot) {
                        p_slot.id = id;
                        p_slot.drop_flag.store(false, Relaxed);

                        debug_assert!({
                            let data = p_slot.data.lock();
                            data.0.is_none() && data.1.is_none()
                        });

                        return slot;
                    } else {
                        // This is a corner case, where the remaining references in the `ReqSlot`
                        // that were returned to the pool by the `drop_flag` have not yet been
                        // released. This rarely happens, but it is not impossible.
                        self.pool.lock().1.push_back(slot);
                    }
                }

                // We couldn't find a slot in the pool, so we have to allocate a new one.
                Arc::new(ReqSlot::new(Arc::downgrade(self), id))
            };

            assert!(self.active.insert(slot.id, slot.clone()).is_none());
            slot
        }

        pub fn try_set_reply(&self, id: u128, result: Reply) -> Option<()> {
            let Some((org_id, node)) = self.active.remove(&id) else { return None };
            debug_assert_eq!(org_id, id, "dumb logic error");
            debug_assert_eq!(node.id, id, "dumb logic error");

            node.set_reply(result);

            if node.drop_flag.swap(true, AcqRel) == true {
                // We're dealing with corner cases. This is when the `future` object is cancelled
                // between pulling the value from the `active` table and setting the `reply`.
                // It is the responsibility of this routine to execute the cleanup logic of the
                // cancelled object.
                //
                // See [`ReqSlot::cancel`] routine for more details.
                *node.data.lock() = (None, None);
                self.pool.lock().1.push_back(node);
            }

            Some(())
        }
    }

    /* ------------------------------------ Request Handling ------------------------------------ */
    /// Contains route to the RPC endpoint, and received payload
    pub struct Request {
        head: raw::HReq,
        data: BufferPtr,

        w_body: Weak<Instance>,
    }

    impl std::fmt::Debug for Request {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Request").finish()
        }
    }

    impl Request {
        pub fn route(&self) -> &[u8] {
            &self.data[self.head.route()]
        }

        pub fn route_cstr(&self) -> Result<&CStr, impl std::error::Error> {
            CStr::from_bytes_with_nul(&self.data[self.head.route_c()])
        }

        pub fn payload(&self) -> &[u8] {
            // Null byte from both ends is excluded.
            &self.data[self.head.payload()]
        }

        pub unsafe fn payload_cstr_unchecked(&self) -> &CStr {
            //! In this function, the last byte is guaranteed to be NULL, but there is no
            //! guarantee that there are no NULL bytes in the middle. To avoid unnecessarily
            //! iterating over all the bytes, the function is marked as unsafe and does not
            //! perform any verification logic.
            CStr::from_bytes_with_nul_unchecked(&self.data[self.head.payload_c()])
        }

        pub fn req_id(&self) -> u128 {
            raw::retrieve_req_id(&self.data[self.head.req_id()])
        }

        pub async fn reply(
            mut self,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            Self::_reply(self._take_body(), raw::RepCode::Okay, payload).await
        }

        pub async fn error(
            mut self,
            errc: raw::RepCode,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            Self::_reply(self._take_body(), errc, payload).await
        }

        pub async fn user_error(
            mut self,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            Self::_reply(self._take_body(), raw::RepCode::UserError, payload).await
        }

        pub async fn user_error_by<T: std::fmt::Display>(self, error: T) -> std::io::Result<()> {
            let e = format!("{}", error);
            self.user_error([e.as_bytes()]).await
        }

        pub async fn error_no_route(mut self) -> std::io::Result<()> {
            Self::_reply(self._take_body(), raw::RepCode::NoRoute, [self.route()]).await
        }

        async fn _reply(
            body: Weak<Instance>,
            errc: raw::RepCode,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            todo!()
        }

        fn _take_body(&mut self) -> Weak<Instance> {
            replace(&mut self.w_body, default())
        }
    }

    impl Drop for Request {
        fn drop(&mut self) {
            //! In the `rpc-it` library, all data transmission is based on direct transmission
            //! polling of the low-level [`crate::AsyncFrameWrite`] handler.
            //!
            //! No I/O operations that rely on `async` or blocking can be performed within
            //! the `Drop` handler, so handling of unprocessed request responses is handed off
            //! to worker tasks.
            self._take_body()
                .upgrade()
                .map(|x| x.tx_aborts.send(self.req_id()));
        }
    }

    /* ------------------------------------- Notify Handling ------------------------------------ */
    pub struct Notify {
        head: raw::HNoti,
        data: BufferPtr,
    }

    impl std::fmt::Debug for Notify {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Notify").field("head", &self.head).finish()
        }
    }

    impl Notify {
        pub fn route(&self) -> &[u8] {
            &self.data[self.head.route()]
        }

        pub fn route_cstr(&self) -> Result<&CStr, impl std::error::Error> {
            CStr::from_bytes_with_nul(&self.data[self.head.route_c()])
        }

        pub fn payload(&self) -> &[u8] {
            // Null byte from both ends is excluded.
            &self.data[self.head.payload()]
        }

        pub unsafe fn payload_cstr_unchecked(&self) -> &CStr {
            //! In this function, the last byte is guaranteed to be NULL, but there is no
            //! guarantee that there are no NULL bytes in the middle. To avoid unnecessarily
            //! iterating over all the bytes, the function is marked as unsafe and does not
            //! perform any verification logic.
            CStr::from_bytes_with_nul_unchecked(&self.data[self.head.payload_c()])
        }
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                        WRITER LOGIC                                        */
    /* ------------------------------------------------------------------------------------------ */

    /// Handles write operation to the underlying transport layer
    pub(crate) struct Instance {
        pub rx_inbound: flume::Receiver<super::Inbound>,
        pub tx_aborts: flume::Sender<u128>,
        pub write: AsyncMutex<Pin<Box<dyn AsyncWrite + Send + Sync + Unpin>>>,
        pub reqs: Arc<ReqTable>,
    }

    impl Instance {}
}
