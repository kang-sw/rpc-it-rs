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

/// Describes why did the write runner stopped.
#[derive(Debug)]
pub enum WriteRunnerExitType {
    AllHandleDropped,

    /// The writer was closed manually
    ManualCloseImmediate,

    /// The writer was closed manually. All the remaining write requests were flushed.
    ManualClose,
}

/// Describes what kind of error occurred during write runner execution.
#[derive(Error, Debug)]
pub enum WriteRunnerError {
    #[error("Failed to close the writer: {0}")]
    WriterCloseFailed(std::io::Error),

    #[error("Failed to flush the writer: {0}")]
    WriterFlushFailed(std::io::Error),

    #[error("Failed to write: {0}")]
    WriteFailed(std::io::Error),
}

// ==== DeferredActionError ====

pub(crate) fn convert_deferred_write_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::BackgroundRunnerClosed,
        // XXX: In future, we should deal with re-sending failed message.
        TrySendError::Full(DeferredDirective::WriteNoti(_)) => TrySendMsgError::ChannelAtCapacity,
        TrySendError::Full(_) => unreachable!(),
    }
}

pub(crate) fn convert_deferred_action_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::BackgroundRunnerClosed,
        TrySendError::Full(_) => TrySendMsgError::ChannelAtCapacity,
    }
}
