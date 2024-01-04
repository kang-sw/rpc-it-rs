use std::sync::Arc;

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::mpsc::error::TrySendError;

use crate::codec::{self, Codec};

use super::driver::DeferredDirective;

#[derive(Debug, Error)]
pub enum SendMsgError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    #[error("Background runner is already closed!")]
    BackgroundRunnerClosed,
}

#[derive(Debug, Error)]
pub enum TrySendMsgError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    #[error("Background runner is already closed!")]
    BackgroundRunnerClosed,

    /// The channel is at capacity.
    ///
    /// It contains the message that was attempted to be sent.
    ///
    /// # NOTE
    ///
    /// Currently, there is no way provided to re-send failed message.
    #[error("Channel is at capacity!")]
    ChannelAtCapacity,
}

#[derive(Debug, Error)]
pub enum ReceiveResponseError {
    #[error("RPC server was closed.")]
    ServerClosed,

    #[error("RPC client was closed.")]
    Shutdown,

    #[error("Server returned an error: {0:?}")]
    ErrorResponse(ErrorResponse),
}

#[derive(Debug)]
pub struct ErrorResponse {
    pub(super) errc: codec::ResponseErrorCode,
    pub(super) codec: Arc<dyn Codec>,
    pub(super) data: Bytes,
}

// ==== DeferredActionError ====

pub(crate) fn convert_deferred_write_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::BackgroundRunnerClosed,
        TrySendError::Full(DeferredDirective::WriteNoti(x)) => TrySendMsgError::ChannelAtCapacity,
        TrySendError::Full(_) => unreachable!(),
    }
}

pub(crate) fn convert_deferred_action_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::BackgroundRunnerClosed,
        TrySendError::Full(_) => TrySendMsgError::ChannelAtCapacity,
    }
}
