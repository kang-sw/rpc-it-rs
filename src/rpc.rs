use self::driver::ReqSlot;
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
    type Output = driver::Reply;

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
    body: Arc<driver::Body>,
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
        todo!()
    }
}

/* ----------------------------------------- Weak Handle ---------------------------------------- */
#[derive(Clone)]
pub struct WeakHandle {
    w_driver: Weak<driver::Body>,
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
        ffi::CStr,
        mem::replace,
        ops::AddAssign,
        pin::Pin,
        sync::{
            atomic::{
                AtomicBool, AtomicU8,
                Ordering::{AcqRel, Relaxed},
            },
            Arc, Weak,
        },
        task::Poll,
    };

    use dashmap::DashMap;
    use derive_new::new;
    use parking_lot::Mutex;

    use crate::{
        alias::{default, AsyncMutex, PoolPtr},
        raw,
        transport::{AsyncFrameRead, AsyncFrameWrite},
    };

    /* ------------------------------------------------------------------------------------------ */
    /*                                            TYPES                                           */
    /* ------------------------------------------------------------------------------------------ */

    /// Commonly used reused payload type alias.
    pub type Buffer = PoolPtr<Vec<u8>>;

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
        write: Box<dyn AsyncFrameWrite>,

        /// Read interface to the underlying transport layer
        read: Box<dyn AsyncFrameRead>,

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

            let d = Body {
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
        weak_body: Weak<Body>,
        tx_inbound: flume::Sender<super::Inbound>,
        rx_aborts: flume::Receiver<u128>,
        read: Pin<Box<dyn AsyncFrameRead>>,
        req_table: Arc<ReqTable>,
    ) -> Result<(), Error> {
        // TODO: Payload allocators for various size request ... (small < 256, medium < 16k, large)

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
        data: Buffer,
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
                table.pool.lock().1.push(self);
            } else if handled == true {
                // The value was set already, thus we have to cleanup the value, and reclaim
                // this entity to the queue back.
                *self.data.lock() = (None, None);
                table.pool.lock().1.push(self);
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
        ) -> Poll<Reply> {
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
                    table.pool.lock().1.push(this);
                }

                Poll::Ready(result)
            } else {
                data.0.replace(cx.waker().clone());
                Poll::Pending
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
        pool: Mutex<(u128, Vec<Arc<ReqSlot>>)>,
    }

    impl ReqTable {
        pub fn alloc(self: &Arc<ReqTable>) -> Arc<ReqSlot> {
            let slot = {
                let (id, slot) = {
                    let mut slots = self.pool.lock();

                    slots.0 += 1;
                    let id = slots.0;

                    (id, slots.1.pop())
                };

                if let Some(mut slot) = slot {
                    let p_slot = Arc::get_mut(&mut slot).expect("logic error");
                    p_slot.id = id;
                    p_slot.drop_flag.store(false, Relaxed);

                    debug_assert!({
                        let data = p_slot.data.lock();
                        data.0.is_none() && data.1.is_none()
                    });

                    slot
                } else {
                    Arc::new(ReqSlot::new(Arc::downgrade(self), id))
                }
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
                self.pool.lock().1.push(node);
            }

            Some(())
        }
    }

    /* ------------------------------------ Request Handling ------------------------------------ */
    /// Contains route to the RPC endpoint, and received payload
    pub struct Request {
        head: raw::HReq,
        data: Buffer,

        w_body: Weak<Body>,
    }

    impl std::fmt::Debug for Request {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Request").finish()
        }
    }

    impl Request {
        pub fn route(&self) -> &str {
            todo!()
        }

        pub fn route_cstr(&self) -> &CStr {
            todo!()
        }

        pub fn payload(&self) -> &[u8] {
            todo!()
        }

        pub fn payload_with_nul(&self) -> &[u8] {
            todo!()
        }

        pub fn req_id(&self) -> u128 {
            todo!()
        }

        pub async fn reply(
            mut self,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            Self::_reply(self._take_body(), raw::RepCode::Okay, payload).await
        }

        pub async fn error(
            mut self,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            Self::_reply(self._take_body(), raw::RepCode::UserError, payload).await
        }

        pub async fn error_with<T: std::fmt::Display>(self, error: T) -> std::io::Result<()> {
            let e = format!("{}", error);
            self.error([e.as_bytes()]).await
        }

        pub async fn error_no_route(mut self) -> std::io::Result<()> {
            Self::_reply(
                self._take_body(),
                raw::RepCode::NoRoute,
                [self.route().as_bytes()],
            )
            .await
        }

        async fn _reply(
            body: Weak<Body>,
            errc: raw::RepCode,
            payload: impl IntoIterator<Item = &[u8]>,
        ) -> std::io::Result<()> {
            todo!()
        }

        fn _take_body(&mut self) -> Weak<Body> {
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
        data: Buffer,
    }

    impl std::fmt::Debug for Notify {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Notify").field("head", &self.head).finish()
        }
    }

    impl Notify {
        pub fn route(&self) -> &str {
            todo!()
        }

        pub fn route_cstr(&self) -> &CStr {
            todo!()
        }

        pub fn payload(&self) -> &[u8] {
            todo!()
        }

        pub fn payload_with_nul(&self) -> &[u8] {
            todo!()
        }
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                     RPC DRIVER INSTANCE                                    */
    /* ------------------------------------------------------------------------------------------ */

    /// Handles write operation to the underlying transport layer
    pub(crate) struct Body {
        pub rx_inbound: flume::Receiver<super::Inbound>,
        tx_aborts: flume::Sender<u128>,
        write: AsyncMutex<Pin<Box<dyn AsyncFrameWrite>>>,
        reqs: Arc<ReqTable>,
    }

    impl Body {}
}
