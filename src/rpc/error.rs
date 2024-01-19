use bytes::Bytes;
use mpsc::TrySendError;
use thiserror::Error;

use crate::codec::{self, Codec};

use super::WriterDirective;

// ========================================================== Send Errors ===|

/// Error that occurs when sending request/notify
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SendMsgError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    #[error("A command channel to background runner was already closed")]
    ChannelClosed,

    #[error("You tried to send request, but the receiver is already expired")]
    ReceiverExpired,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SendResponseError {
    #[error("Sending message failed")]
    MsgError(#[from] SendMsgError),

    #[error("This inbound is not request / or it is already responded")]
    InboundNotRequest,
}

/// Error that occurs when trying to send request/notify
#[derive(Debug, Error)]
#[non_exhaustive]
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

    #[error("You tried to send request, but the receiver is already expired")]
    ReceiverExpired,
}

#[derive(Debug, Error)]
pub enum TrySendResponseError {
    #[error("Sending message failed")]
    MsgError(#[from] TrySendMsgError),

    #[error("This inbound is not request / or it is already responded")]
    InboundNotRequest,
}

// ========================================================== TryRecvResponseError ===|

#[derive(Error, Debug)]
pub enum ResponseReceiveError<C: Codec> {
    #[error("A receive channel was closed. No more message to consume!")]
    Closed,

    #[error("No message available")]
    Empty,

    #[error("Already retrieved the result.")]
    Retrieved,

    #[error("Remote peer respond with an error")]
    Response(ErrorResponse<C>),
}

impl<C: Codec> ResponseReceiveError<C> {
    pub fn into_response(self) -> Option<ErrorResponse<C>> {
        match self {
            Self::Response(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_response(&self) -> Option<&ErrorResponse<C>> {
        match self {
            Self::Response(e) => Some(e),
            _ => None,
        }
    }
}

// ==== Utils for deferred ====

pub(crate) fn convert_deferred_write_err(e: TrySendError<WriterDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::ChannelClosed,
        // XXX: In future, we should deal with re-sending failed message.
        TrySendError::Full(
            WriterDirective::WriteMsg(..)
            | WriterDirective::WriteMsgBurst(..)
            | WriterDirective::WriteReqMsg(..),
        ) => TrySendMsgError::ChannelAtCapacity,
        TrySendError::Full(_) => unreachable!(),
    }
}

pub(crate) fn convert_deferred_action_err(e: TrySendError<WriterDirective>) -> TrySendMsgError {
    match e {
        TrySendError::Closed(_) => TrySendMsgError::ChannelClosed,
        TrySendError::Full(_) => TrySendMsgError::ChannelAtCapacity,
    }
}

// ========================================================== Recv Errors ===|

/// Error when trying to receive an inbound message
#[derive(Debug, Error)]
pub enum TryRecvError {
    #[error("A receive channel was closed. No more message to consume!")]
    Closed,

    #[error("No message available")]
    Empty,
}

#[derive(Debug)]
pub struct ErrorResponse<C: Codec> {
    pub(super) errc: codec::ResponseError,
    pub(super) codec: C,
    pub(super) payload: Bytes,
}

// ========================================================== Runner Enums ===|

/// Result of the write runner
pub type WriteRunnerResult = Result<WriteRunnerExitType, WriteRunnerError>;

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
pub enum ReadRunnerExitType {
    AllHandleDropped,
    Eof,
}
