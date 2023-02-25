//! # Components
//!
//! - [`transport`] Defines data transport layer.
//! - [`rpc`] Defines RPC layer
//!
pub use rpc::driver::InitInfo;
pub use rpc::Handle;

pub(crate) mod raw {
    /// Every message starts with this header
    pub const IDENT: [u8; 4] = [b'+', b'@', b'C', b'|'];

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct MessageHeaderRaw {
        /// Check if this is a valid message
        ident: [u8; 4],

        /// 2 bits - message type, 4 bits - reserved, 10 bits - length of route
        front: u16,

        /// - NOTIFY: reserved
        /// - REQUEST: 6bits - reserved, 10 bits - length of request id
        /// - REPLY: reserved
        ext: u16,

        /// Length of the message payload
        payload_len: u32,
    }

    #[derive(Clone, Copy, Debug)]
    pub enum MessageHeader {
        Notify(MsgHead),
        Request(ReqHead),
        Reply(MsgHead),
    }

    #[derive(Clone, Copy, Debug)]
    pub struct ReqHead {
        pub len_route: u16,
        pub len_request_id: u16,
        pub len_payload: u32,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct MsgHead {
        /// If it's [`MessageHeader::Notify`], this field is length of the route.
        /// Otherwise(in case of [`MessageHeader::Reply`]), it's length of the reply id.
        pub len_id: u16,
        pub len_payload: u32,
    }
}

pub mod transport {
    use std::pin::Pin;

    ///
    /// A message writer trait, which is used to write message to the underlying transport
    /// layer.
    ///
    pub trait AsyncFrameWrite: Send + Sync + Unpin {
        /// Called before writing a message. This should clean all internal in-progress message
        /// transport state.
        fn begin_write_msg(&mut self) -> std::io::Result<()>;

        /// Returns OK only when all the requested data is written to the underlying transport
        /// layer.
        fn poll_write_all<'cx, 'b>(
            self: Pin<&mut Self>,
            cx: &'cx mut std::task::Context<'cx>,
            bufs: &'b [&'b [u8]],
        ) -> std::task::Poll<std::io::Result<()>>;
    }

    ///
    /// A message reader trait. This is used to read message from the underlying transport layer.
    ///
    pub trait AsyncFrameRead: Send + Sync + Unpin {
        /// Called before reading a message. This should clean all internal in-progress message
        /// transport state.
        fn begin_read_msg(&mut self) -> std::io::Result<()>;

        /// Returns OK only when all the requested data is read from the underlying transport.
        ///
        /// Unless the driver instance is disposed of after this method is called,
        /// the [`AsyncFrameRead::poll_read_body`] method is guaranteed to be called before
        /// next [`AsyncFrameRead::begin_read_msg`] call.
        ///
        /// Argument `buf` is likely to be the same as the size of [`crate::raw::MessageHeaderRaw`]
        fn poll_read_head<'this, 'cx, 'b: 'this>(
            self: Pin<&'this mut Self>,
            cx: &'cx mut std::task::Context<'cx>,
            buf: &'b mut [u8],
        ) -> std::task::Poll<std::io::Result<()>>;

        /// Returns OK only when all the requested data is read from the underlying transport
        /// layer. In all other cases returns Err.
        ///
        /// The lifetime of the supplied buffer argument is guaranteed to be longer than the
        /// lifetime of the instance of this trait object.
        fn poll_read_body<'this, 'cx, 'b: 'this>(
            self: Pin<&'this mut Self>,
            cx: &'cx mut std::task::Context<'cx>,
            buf: &'b mut [u8],
        ) -> std::task::Poll<std::io::Result<()>>;
    }
}

pub mod alias {
    use lockfree_object_pool::{LinearObjectPool, LinearOwnedReusable, LinearReusable};
    use std::sync::Arc;

    pub type PoolPtr<T> = LinearOwnedReusable<T>;
    pub type PoolRef<'a, T> = LinearReusable<'a, T>;
    pub type Pool<T> = Arc<LinearObjectPool<T>>;
}

pub mod rpc {
    use std::sync::Arc;

    /* ------------------------------------------------------------------------------------------ */
    /*                                          RPC TYPES                                         */
    /* ------------------------------------------------------------------------------------------ */
    #[derive(Debug)]
    pub struct Request {}

    #[derive(Debug)]
    pub struct Notify {}

    #[derive(Debug, thiserror::Error)]
    pub enum DriverError {
        #[error("unknown error")]
        Unknown,
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                         RPC DRIVER                                         */
    /* ------------------------------------------------------------------------------------------ */
    pub mod driver {
        use std::num::NonZeroUsize;

        use crate::transport::{AsyncFrameRead, AsyncFrameWrite};

        #[derive(typed_builder::TypedBuilder)]
        pub struct InitInfo {
            /// Write interface to the underlying transport layer
            pub write: Box<dyn AsyncFrameWrite>,

            /// Read interface to the underlying transport layer
            pub read: Box<dyn AsyncFrameRead>,

            /// Number of maximum buffered notifies that can be queued in buffer.
            /// If this limit is reached, the driver will discard further notifies from client.
            ///
            /// Specifying None here will make the notify buffer unbounded.
            #[builder(default)]
            pub notify_buffer_size: Option<NonZeroUsize>,

            /// Number of maximum buffered requests that can be queued in buffer.
            /// If this limit is reached, the driver will discard further requests from client,
            /// and will return an error to the client.
            ///
            /// Specifying None here will make the request buffer unbounded.
            #[builder(default)]
            pub request_buffer_size: Option<NonZeroUsize>,
        }

        pub(crate) struct Instance {
            init: InitInfo,
        }
    }

    /* ------------------------------------------------------------------------------------------ */
    /*                                         RPC HANDLE                                         */
    /* ------------------------------------------------------------------------------------------ */
    ///
    /// Handle to control overall RPC operation
    ///
    pub struct Handle {
        driver: Arc<driver::Instance>,
    }

    /// Type alias for RPC driver
    #[derive(Clone, Debug)]
    pub struct RequestSource(flume::Receiver<Request>);

    #[derive(Clone, Debug)]
    pub struct NotifySource(flume::Receiver<Notify>);

    impl Handle {
        pub fn clone_request_source(&self) -> RequestSource {
            todo!()
        }

        pub fn clone_notify_source(&self) -> NotifySource {
            todo!()
        }

        /// Send a request to the remote end, and returns waitable reply.
        pub async fn request(
            &self,
            route: &str,
            payload: impl IntoIterator,
        ) -> std::io::Result<ReplyWait> {
            todo!()
        }

        pub async fn notify(&self, route: &str, payload: impl IntoIterator) -> std::io::Result<()> {
            todo!()
        }
    }

    /* ------------------------------------------ Reply ----------------------------------------- */
    #[derive(Debug)]
    pub struct ReplyWait {}
}
