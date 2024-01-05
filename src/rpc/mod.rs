use std::borrow::Cow;

use std::sync::Arc;
use std::sync::Weak;

use bytes::BytesMut;
use tokio::sync::mpsc;

use crate::codec::error::EncodeError;
use crate::codec::Codec;
use crate::defs::RequestId;

use self::error::*;
use self::req_rep::*;

pub use self::receiver::*;
pub use self::sender::*;

pub use self::builder::create_builder;
pub use self::req_rep::Response;

// ==== Basic RPC ====

/// Generic trait for underlying RPC connection.
///
/// It hides complicated generic types and provides a unified interface for RPC connection.
pub(crate) trait RpcCore<U: UserData>: std::fmt::Debug {
    fn self_as_codec(self: Arc<Self>) -> Arc<dyn Codec>;
    fn codec(&self) -> &dyn Codec;
    fn user_data(&self) -> &U;

    /// Borrow `tx_deferred` channel
    fn tx_deferred(&self) -> &mpsc::Sender<DeferredDirective>;

    /// Only available for [`RequestSender`].
    fn shutdown_rx_channel(&self);

    /// Increments the reference count of the [`RequestSender`] handle.
    fn incr_request_sender_refcnt(&self);

    /// Decrements the reference count of the [`RequestSender`] handle. If all request senders are
    /// dropped, the background writer's response handler side will be shut down.
    fn decr_request_sender_refcnt(&self);
}

/// A trait constraint for user data type of a RPC connection.
pub trait UserData: std::fmt::Debug + Send + Sync + 'static {}
impl<T> UserData for T where T: std::fmt::Debug + Send + Sync + 'static {}

// ========================================================== Details ===|

pub mod builder;
pub mod error;
mod req_rep;
mod sender;
mod receiver {
    use std::{borrow::Cow, sync::Arc};

    use bytes::Bytes;
    use tokio::sync::mpsc;

    use crate::{defs::RangeType, Codec, ParseMessage, UserData};

    use super::RpcCore;

    /// Handles error during receiving inbound messages inside runner.
    pub trait ReceiveErrorHandler {}

    /// A receiver which deals with inbound notifies / requests.
    #[derive(Debug)]
    pub struct Receiver<U> {
        context: Arc<dyn RpcCore<U>>,
        channel: mpsc::Receiver<InboundInner>,
    }

    pub(crate) enum InboundInner {
        Request {
            buffer: Bytes,
            method: RangeType,
            payload: RangeType,
            req_id: RangeType,
        },
        Notify {
            buffer: Bytes,
            method: RangeType,
            payload: RangeType,
        },
    }

    // ==== impl:Receiver ====

    impl<U> Receiver<U> {}

    // ========================================================== Inbound ===|

    /// A inbound message that was received from remote. It is either a notification or a request.
    pub struct Inbound<'a, U> {
        owner: Cow<'a, Arc<dyn RpcCore<U>>>,
        inner: InboundInner,
    }

    // ==== impl:Inbound ====
    impl<'a, U> Inbound<'a, U>
    where
        U: UserData,
    {
        pub fn user_data(&self) -> &U {
            self.owner.user_data()
        }

        pub fn codec(&self) -> &dyn Codec {
            self.owner.codec()
        }

        pub fn into_owned(self) -> Inbound<'static, U> {
            Inbound {
                owner: Cow::Owned(Arc::clone(&self.owner)),
                inner: self.inner,
            }
        }

        fn payload_bytes(&self) -> &[u8] {
            let (b, p) = match &self.inner {
                InboundInner::Request {
                    buffer, payload, ..
                } => (buffer, payload),
                InboundInner::Notify {
                    buffer, payload, ..
                } => (buffer, payload),
            };

            &b[p.range()]
        }

        pub fn is_request(&self) -> bool {
            matches!(&self.inner, InboundInner::Request { .. })
        }

        // TODO: response_ok

        // TODO: response_error

        // TODO: response_error_with

        // TODO: drop_request = drops request without sending any response.
    }

    impl<'a, U> ParseMessage for Inbound<'a, U>
    where
        U: UserData,
    {
        fn codec_payload_pair(&self) -> (&dyn Codec, &[u8]) {
            (self.owner.codec(), self.payload_bytes())
        }
    }
}
