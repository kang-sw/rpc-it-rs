use std::sync::Arc;

use bytes::Bytes;
use thiserror::Error;
use tokio::sync::mpsc::error::TrySendError;

use crate::codec::{self, Codec};

use super::driver::DeferredDirective;

#[derive(Debug, Error)]
pub enum SendError {
    #[error("Encoding failed: {0}")]
    EncodeFailed(#[from] codec::error::EncodeError),

    /// This won't be returned calling `*_deferred` methods.
    #[error("Async IO failed: {0}")]
    AsyncIoError(#[from] std::io::Error),

    /// This won't be returned calling async methods.
    #[error("Failed to send request to background driver: {0}")]
    DeferredIoError(#[from] DeferredActionError<Bytes>),
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("Send failed: {0}")]
    SendFailed(#[from] SendError),
}

#[derive(Debug, Error)]
pub enum DeferredActionError<T> {
    #[error("Background runner is already closed!")]
    BackgroundRunnerClosed,

    #[error("Channel is at capacity!")]
    ChannelAtCapacity(T),
}

#[derive(Debug, Error)]
pub enum ReceiveResponseError {
    #[error("RPC server was closed.")]
    ServerClosed,

    #[error("RPC client was closed.")]
    ClientClosed,

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

pub(crate) fn convert_deferred_write_err(
    e: TrySendError<DeferredDirective>,
) -> DeferredActionError<Bytes> {
    match e {
        TrySendError::Closed(_) => DeferredActionError::BackgroundRunnerClosed,
        TrySendError::Full(DeferredDirective::WriteNoti(x)) => {
            DeferredActionError::ChannelAtCapacity(x)
        }
        TrySendError::Full(_) => unreachable!(),
    }
}

pub(crate) fn convert_deferred_action_err(
    e: TrySendError<DeferredDirective>,
) -> DeferredActionError<()> {
    match e {
        TrySendError::Closed(_) => DeferredActionError::BackgroundRunnerClosed,
        TrySendError::Full(_) => DeferredActionError::ChannelAtCapacity(()),
    }
}
