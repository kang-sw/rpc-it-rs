use std::sync::Arc;

use bytes::Bytes;
use mpsc::TrySendError;
use thiserror::Error;

use crate::codec::{self, Codec};

use super::DeferredDirective;

/// Error that occurs when sending request/notify
#[derive(Debug, Error)]
pub enum SendMsgError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    #[error("A command channel to background runner was already closed")]
    ChannelClosed,
}

#[derive(Debug, Error)]
pub enum SendResponseError {
    #[error("Sending message failed")]
    MsgError(#[from] SendMsgError),

    #[error("This inbound is not request / or it is already responded")]
    InboundNotRequest,
}

/// Error that occurs when trying to send request/notify
#[derive(Debug, Error)]
pub enum TrySendMsgError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    #[error("A command channel to background runner was already closed")]
    ChannelClosed,

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

/// Error when trying to receive an inbound message
#[derive(Debug, Error)]
pub enum TryRecvError {
    #[error("A receive channel was closed. No more message to consume!")]
    Closed,

    #[error("No message available")]
    Empty,
}

#[derive(Debug, Error)]
pub enum TrySendResponseError {
    #[error("Sending message failed")]
    MsgError(#[from] TrySendMsgError),

    #[error("This inbound is not request / or it is already responded")]
    InboundNotRequest,
}

/// Error that occurs when receiving response
#[derive(Debug, Error)]
pub enum ReceiveResponseError {
    #[error("RPC service was disposed.")]
    Disconnected,

    #[error("Server returned an error: {0:?}")]
    ErrorResponse(ErrorResponse),
}

#[derive(Debug)]
pub struct ErrorResponse {
    pub(super) errc: codec::ResponseError,
    pub(super) codec: Arc<dyn Codec>,
    pub(super) payload: Bytes,
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

/// Describes why did the read runner stopped.
#[derive(Debug)]
pub enum ReadRunnerExitType {}

/// Describes what kind of error occurred during read runner execution.
#[derive(Error, Debug)]
pub enum ReadRunnerError {}

// ==== DeferredActionError ====

pub(crate) fn convert_deferred_write_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::ChannelClosed,
        // XXX: In future, we should deal with re-sending failed message.
        TrySendError::Full(DeferredDirective::WriteMsg(_)) => TrySendMsgError::ChannelAtCapacity,
        TrySendError::Full(_) => unreachable!(),
    }
}

pub(crate) fn convert_deferred_action_err(e: TrySendError<DeferredDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::ChannelClosed,
        TrySendError::Full(_) => TrySendMsgError::ChannelAtCapacity,
    }
}
