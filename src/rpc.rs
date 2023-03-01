use derive_more::From;
use smallvec::SmallVec;

use crate::{
    alias::default,
    consts::SMALL_PAYLOAD_SLICE_COUNT,
    raw::{self, IdType, ID_MAX_LEN, MAX_ROUTE_LEN},
    transport,
};

use self::driver::ReqSlot;
use std::{
    future::poll_fn,
    io::IoSlice,
    pin::Pin,
    sync::{Arc, Weak},
};

/// Magic number: Determine the maximum number of payload arrays to stack.
pub type SmallPayloadVec<'a> = SmallVec<[IoSlice<'a>; SMALL_PAYLOAD_SLICE_COUNT]>;

/* ------------------------------------------------------------------------------------------ */
/*                                          RPC TYPES                                         */
/* ------------------------------------------------------------------------------------------ */
#[derive(Debug, From)]
pub enum Inbound {
    Request(driver::Request),
    Notify(driver::Notify),
}

/* ------------------------------------- Reply Handler Type ------------------------------------- */
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ReplyWait {
    slot: Option<Arc<ReqSlot>>,
}

impl ReplyWait {
    pub fn request_id(&self) -> IdType {
        self.slot.as_ref().unwrap().id
    }
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

#[cfg(feature = "futures")]
impl futures::future::FusedFuture for ReplyWait {
    fn is_terminated(&self) -> bool {
        self.slot.is_none()
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

    /// Flush all queued write requests. This is simply a function that forwards flush requests
    /// to the internal transport layer.
    pub async fn flush(&self) -> std::io::Result<()> {
        let mut write = self.body.write.lock().await;
        poll_fn(|cx| Pin::new(&mut **write).poll_flush(cx)).await
    }

    /// Shutdown underlying write stream.
    pub async fn shutdown(&self) -> std::io::Result<()> {
        let mut write = self.body.write.lock().await;
        poll_fn(|cx| Pin::new(&mut **write).poll_shutdown(cx)).await
    }

    /// Number of pending requests.
    pub fn pending_requests(&self) -> usize {
        self.body.reqs.pending_requests()
    }

    /// Send a request to the remote end, and returns awaitable reply.
    pub async fn request<'a, T: AsRef<[u8]> + 'a + ?Sized>(
        &self,
        route: &str,
        payload: impl IntoIterator<Item = &'a T>,
    ) -> std::io::Result<ReplyWait> {
        let (_, wait) = self
            .request_n_bytes(route, &mut payload.into_iter().map(|x| x.as_ref()))
            .await?;
        Ok(wait)
    }

    /// Send a notify to the remote end. This will return after writing all payload to underlying
    /// transport.
    pub async fn notify<'a, T: AsRef<[u8]> + 'a + ?Sized>(
        &self,
        route: &str,
        payload: impl IntoIterator<Item = &'a T>,
    ) -> std::io::Result<usize> {
        self._notify(route, &mut payload.into_iter().map(|x| x.as_ref()))
            .await
    }

    /// Send a request to the remote end, and returns awaitable reply.
    ///
    /// You can emulate timeout behavior by dropping returned future.
    ///
    /// This version returns the number of bytes written.
    pub async fn request_n_bytes(
        &self,
        route: &str,
        payload: &mut impl Iterator<Item = &[u8]>,
    ) -> std::io::Result<(usize, ReplyWait)> {
        assert!(route.len() <= MAX_ROUTE_LEN);

        // Try allocate reply slot first
        let slot = self.body.reqs.alloc();
        let id = slot.id;

        // Making this let slot canceled automatically on write error.
        let wait = ReplyWait { slot: Some(slot) };

        // Create notification header
        let header_buf;
        let mut id_buf: [u8; ID_MAX_LEN] = default();
        let id_buf = raw::store_req_id(id, &mut id_buf);

        // Naively assume that the payload array is small enough to fit in stack.
        //  - First element is reserved for header.
        //  - Second element is reserved for route.
        //  - Third element is zero.
        let mut b = SmallPayloadVec::new();
        b.extend(
            [&[], id_buf, route.as_bytes(), &[0u8]]
                .iter()
                .map(|x| IoSlice::new(x)),
        );
        b.extend(
            payload
                .filter(|x| x.is_empty() == false)
                .map(|x| IoSlice::new(x.as_ref())),
        );

        // Write header
        let total_len = b[1..].iter().map(|x| x.len()).sum::<usize>();
        let header = raw::Head::Req(raw::HReq {
            n_route: route.len() as u16,
            n_all: total_len as u32,
            n_req_id: id_buf.len() as u8,
        });
        header_buf = raw::RawHead::new(header).to_bytes();
        b[0] = IoSlice::new(&header_buf);

        // Write to transport
        let mut write = self.body.write.lock().await;
        let nw = transport::util::write_vectored_all(&mut **write, &mut b).await?;

        Ok((nw, wait))
    }

    async fn _notify(
        &self,
        route: &str,
        payload: &mut impl Iterator<Item = &[u8]>,
    ) -> std::io::Result<usize> {
        assert!(route.len() <= MAX_ROUTE_LEN);

        // Create notification header
        let header_buf;

        // Naively assume that the payload array is small enough to fit in stack.
        //  - First element is reserved for header.
        //  - Second element is reserved for route.
        //  - Third element is zero.
        let mut b = SmallPayloadVec::new();
        b.extend(
            [&[], route.as_bytes(), &[0u8]]
                .iter()
                .map(|x| IoSlice::new(x)),
        );
        b.extend(
            payload
                .filter(|x| x.is_empty() == false)
                .map(|x| IoSlice::new(x.as_ref())),
        );

        // Write header
        let total_len = b[1..].iter().map(|x| x.len()).sum::<usize>();
        let header = raw::Head::Noti(raw::HNoti {
            n_route: route.len() as u16,
            n_all: total_len as u32,
        });
        header_buf = raw::RawHead::new(header).to_bytes();
        b[0] = IoSlice::new(&header_buf);

        // Write to transport
        let mut write = self.body.write.lock().await;
        transport::util::write_vectored_all(&mut **write, &mut b).await
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
        io::IoSlice,
        mem::replace,
        pin::Pin,
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
    use derive_more::From;
    use derive_new::new;
    use flume::TrySendError;
    use futures_util::{select, FutureExt};
    use parking_lot::Mutex;

    use crate::{
        alias::{default, AsyncMutex, Pool, PoolPtr},
        consts::{BUFSIZE_LARGE, BUFSIZE_SMALL},
        raw::{self, IdType, ID_MAX_LEN},
        transport::{self, AsyncFrameRead, AsyncFrameWrite},
        Handle,
    };

    use super::SmallPayloadVec;

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

        #[error("driver is disposed")]
        Disposed,

        #[error("Failed to parse header: {0}")]
        UnkownType(#[from] raw::ParseError),
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
        /// Specifying maximum here(default) will make the notify buffer unbounded.
        #[builder(default = usize::MAX)]
        inbound_channel_size: usize,

        /// If this flag is set, when the inbound channel is fulfilled, the driver will
        /// be blocked until any of the inbound message are processed.
        #[builder(default = false)]
        block_on_full_inbound: bool,

        /// Set the buffer pooling steps. Buffers larger than the largest buffer size will be
        /// shrunk back to the size specified by `largest_buffer_shrink_size` at the end of use.
        ///
        /// The default setting performs pooling for buffers smaller than 128 bytes and 4096 bytes,
        /// and always frees memory for buffers larger than that.
        #[builder(default=vec![BUFSIZE_SMALL, BUFSIZE_LARGE])]
        buffer_size_levels: Vec<usize>,

        /// If a buffer larger than this value is allocated, it will be shrunk to the size
        /// specified by this value when the buffer is checked into the pool. Default value '0'
        /// clears buffer allocation every time.
        ///
        /// To disable this feature, set this value to usize::MAX.
        #[builder(default = 0)]
        largest_buffer_shrink_size: usize,
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                         DRIVER TASK                                        */
    /* ------------------------------------------------------------------------------------------ */
    impl InitInfo {
        pub fn start(
            self,
        ) -> (
            super::Handle,
            impl std::future::Future<Output = Result<(), Error>>,
        ) {
            let (tx_aborts, rx_aborts) = flume::unbounded();
            let (tx_inbound, rx_inbound) = (self.inbound_channel_size == usize::MAX)
                .then(|| flume::unbounded())
                .unwrap_or_else(|| flume::bounded(self.inbound_channel_size));

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

            let mut buffer_sizes = self.buffer_size_levels;
            buffer_sizes.sort();
            buffer_sizes.dedup();

            let mut allocs: Vec<_> = buffer_sizes
                .into_iter()
                .map(|x| {
                    (
                        x,
                        Arc::new(Pool::new(
                            move || Vec::with_capacity(x),
                            |x| unsafe { x.set_len(0) },
                        )),
                    )
                })
                .collect();

            // Setup fallback allocation
            allocs.push((
                usize::MAX,
                Arc::new(Pool::new(
                    || Vec::new(),
                    move |x| unsafe {
                        x.set_len(0);
                        x.shrink_to(self.largest_buffer_shrink_size);
                    },
                )),
            ));

            (
                h,
                Driver::new(
                    weak,
                    tx_inbound,
                    rx_aborts,
                    read,
                    reqs,
                    allocs.into(),
                    self.block_on_full_inbound,
                )
                .run(),
            )
        }
    }

    /* ---------------------------------- Driver Implementation --------------------------------- */
    /// Receive data from the remote end. For the `notify` and `request` types, create a
    /// receive object, push it to the channel, and route the `reply` type to the entity
    /// waiting for a response as appropriate.
    ///
    /// Sending cancellation handling for cancelled request handlers issued by the `Drop`
    /// routine is also performed by the driver.
    ///
    #[derive(derive_new::new)]
    struct Driver {
        weak_body: Weak<Instance>,
        tx_inbound: flume::Sender<super::Inbound>,
        rx_aborts: flume::Receiver<IdType>,
        read: Pin<Box<dyn AsyncFrameRead>>,
        req_table: Arc<ReqTable>,
        allocs: VarSizePool,
        block_on_full: bool,
    }

    impl Driver {
        pub async fn run(mut self) -> Result<(), Error> {
            let mut task_inbound = Box::pin(Self::read_ops(
                &mut *self.read,
                &self.weak_body,
                &self.tx_inbound,
                &self.req_table,
                &self.allocs,
                self.block_on_full,
            ))
            .fuse();

            let result = loop {
                select! {
                    result = &mut task_inbound => break result,

                    abort = self.rx_aborts.recv_async() => {
                        match (abort, self.weak_body.upgrade() ) {
                            (Ok(id), Some(body)) => {
                                body.write_reply(id, crate::RepCode::Aborted, &mut <[&[u8]; 0]>::default().into_iter()).await?;
                            }
                            _ => {
                                // Means `Instance` has been disposed.
                                break Err(Error::Disposed)
                            }
                        }
                    }
                };
            };

            // Just try to shutdown the connection, if possible.
            if let Some(body) = self.weak_body.upgrade() {
                // It's okay to fail this operation, as we might be exitting
                // due to previous shutdown.
                Handle { body }.shutdown().await.ok();
            }

            result
        }

        pub async fn read_ops(
            read: &mut dyn AsyncFrameRead,
            weak_body: &Weak<Instance>,
            tx: &flume::Sender<super::Inbound>,
            req_table: &Arc<ReqTable>,
            allocs: &VarSizePool,
            block_on_full: bool,
        ) -> Result<(), Error> {
            use raw::*;
            use transport::util::read_all;

            let mut hbuf: RawHeadBuf = default();

            loop {
                read_all(read, &mut hbuf).await?;
                let head = RawHead::from_bytes(hbuf);

                match head.parse()? {
                    Head::Noti(head) => {
                        let mut data = allocs.allocate(head.n_all as usize);
                        read_all(read, &mut data[..]).await?;
                        data.push(0);

                        let msg = Notify { head, data }.into();

                        if block_on_full {
                            tx.send_async(msg).await.map_err(|_| Error::Disposed)?;
                        } else {
                            match tx.try_send(msg) {
                                Ok(_) | Err(TrySendError::Full(_)) => (), // Discard if channel is full
                                Err(TrySendError::Disconnected(_)) => break Err(Error::Disposed),
                            }
                        }
                    }

                    Head::Req(head) => {
                        let mut data = allocs.allocate(head.n_all as usize);
                        read_all(read, &mut data[..]).await?;
                        data.push(0);

                        let msg = Request {
                            head,
                            data,
                            w_body: weak_body.clone(),
                        }
                        .into();

                        if block_on_full {
                            tx.send_async(msg).await.map_err(|_| Error::Disposed)?;
                        } else {
                            match tx.try_send(msg) {
                                Ok(_) | Err(TrySendError::Full(_)) => (), // Discard if channel is full
                                Err(TrySendError::Disconnected(_)) => break Err(Error::Disposed),
                            }
                        }
                    }

                    Head::Rep(head) => {
                        let mut data = allocs.allocate(head.n_all as usize);
                        read_all(read, &mut data[..]).await?;
                        data.push(0);

                        let rep = Reply { head, data };
                        let id = rep.request_id();

                        // Just don't handle the error, as user simply dropped the
                        // `ReplyWait` object.
                        req_table.try_set_reply(id, rep);
                    }
                }
            }
        }
    }

    /* -------------------------------- Memory Allocator Utility -------------------------------- */
    #[derive(From)]
    pub struct VarSizePool(Vec<(usize, Arc<Pool<Vec<u8>>>)>);

    impl VarSizePool {
        pub fn allocate(&self, len: usize) -> BufferPtr {
            let idx = self
                .0
                .binary_search_by_key(&(len + 1), |x| x.0)
                .unwrap_or_else(|x| x);

            let (_, pool) = &self.0[idx];
            let mut buf = pool.pull_owned();

            unsafe {
                buf.set_len(0);
                buf.reserve(len + 1); // for null byte
                buf.set_len(len);
            }

            buf
        }
    }

    /* ------------------------------------- Reply Structure ------------------------------------ */
    pub struct Reply {
        head: raw::HRep,
        data: BufferPtr,
    }

    impl Reply {
        pub fn errc(&self) -> crate::RepCode {
            self.head.errc
        }

        pub fn request_id(&self) -> IdType {
            raw::retrieve_req_id(&self.data[self.head.req_id()])
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

    /* -------------------------------- Request - Reply Handling -------------------------------- */
    /// Reusable instance of RPC slot.
    pub(crate) struct ReqSlot {
        table: Weak<ReqTable>,
        pub id: IdType,

        drop_flag: AtomicBool,
        data: Mutex<(Option<std::task::Waker>, Option<Reply>)>,
    }
    impl ReqSlot {
        pub fn new(table: Weak<ReqTable>, id: IdType) -> Self {
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
            } else if let Some(_table) = this.table.upgrade() {
                // As long as the table instance is alive, we'll be notified for any event ...
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
        active: DashMap<IdType, Arc<ReqSlot>>,

        /// (id_gen, idle_slots)
        #[new(default)]
        pool: Mutex<(IdType, VecDeque<Arc<ReqSlot>>)>,
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
        pub fn pending_requests(&self) -> usize {
            self.active.len()
        }

        pub fn alloc(self: &Arc<ReqTable>) -> Arc<ReqSlot> {
            let slot = 'outer: {
                let (id, slot) = {
                    let mut slots = self.pool.lock();
                    let (id_gen, slots) = &mut *slots;

                    *id_gen = id_gen.wrapping_add(1);
                    (*id_gen, slots.pop_front())
                };

                if let Some(mut slot) = slot {
                    if let Some(p_slot) = Arc::get_mut(&mut slot) {
                        p_slot.id = id;
                        p_slot.drop_flag.store(false, Relaxed);

                        debug_assert!({
                            let data = p_slot.data.lock();
                            data.0.is_none() && data.1.is_none()
                        });

                        break 'outer slot;
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

        pub fn try_set_reply(&self, id: IdType, result: Reply) -> Option<()> {
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

        pub fn request_id(&self) -> IdType {
            raw::retrieve_req_id(&self.data[self.head.req_id()])
        }

        pub async fn reply<'a, T: AsRef<[u8]> + ?Sized + 'a>(
            mut self,
            payload: impl IntoIterator<Item = &'a T>,
        ) -> std::io::Result<usize> {
            Self::_reply(
                self._take_body(),
                self.request_id(),
                raw::RepCode::Okay,
                payload,
            )
            .await
        }

        pub async fn error<'a, T: AsRef<[u8]> + ?Sized + 'a>(
            mut self,
            errc: raw::RepCode,
            payload: impl IntoIterator<Item = &'a T>,
        ) -> std::io::Result<usize> {
            Self::_reply(self._take_body(), self.request_id(), errc, payload).await
        }

        pub async fn user_error<'a, T: AsRef<[u8]> + ?Sized + 'a>(
            mut self,
            payload: impl IntoIterator<Item = &'a T>,
        ) -> std::io::Result<usize> {
            Self::_reply(
                self._take_body(),
                self.request_id(),
                raw::RepCode::UserError,
                payload,
            )
            .await
        }

        pub async fn user_error_by<T: std::fmt::Display>(self, error: T) -> std::io::Result<usize> {
            let e = format!("{}", error);
            self.user_error([e.as_bytes()]).await
        }

        pub async fn error_no_route(mut self) -> std::io::Result<usize> {
            Self::_reply(
                self._take_body(),
                self.request_id(),
                raw::RepCode::NoRoute,
                [self.route()],
            )
            .await
        }

        async fn _reply<'a, T: AsRef<[u8]> + 'a + ?Sized>(
            body: Weak<Instance>,
            id: IdType,
            errc: raw::RepCode,
            payload: impl IntoIterator<Item = &'a T>,
        ) -> std::io::Result<usize> {
            let body = body.upgrade().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "underlying instance has been dropped",
                )
            })?;

            body.write_reply(id, errc, &mut payload.into_iter().map(|x| x.as_ref()))
                .await
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
                .map(|x| x.tx_aborts.send(self.request_id()));
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
        pub write: AsyncMutex<Pin<Box<dyn AsyncFrameWrite>>>,
        pub reqs: Arc<ReqTable>,

        tx_aborts: flume::Sender<IdType>,
    }

    impl Instance {
        pub async fn write_reply(
            &self,
            id: IdType,
            errc: raw::RepCode,
            payload: &mut impl Iterator<Item = &[u8]>,
        ) -> std::io::Result<usize> {
            let head_buf;
            let mut id_buf: [u8; ID_MAX_LEN] = default();
            let id_buf = raw::store_req_id(id, &mut id_buf);

            let mut b = SmallPayloadVec::new();
            b.extend([&[], id_buf].iter().map(|x| IoSlice::new(x)));
            b.extend(
                payload
                    .filter(|x| x.is_empty() == false)
                    .map(|x| IoSlice::new(x.as_ref())),
            );

            let total_bytes: usize = b.iter().map(|x| x.len()).sum();
            let head = raw::HRep {
                n_all: total_bytes as u32,
                errc,
                n_req_id: id_buf.len() as u8,
            };
            head_buf = raw::RawHead::new(head.into()).to_bytes();
            b[0] = IoSlice::new(&head_buf);

            let mut write = self.write.lock().await;
            transport::util::write_vectored_all(&mut **write, &mut b).await
        }
    }
}
