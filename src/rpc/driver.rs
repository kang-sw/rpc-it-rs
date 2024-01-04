use bytes::Bytes;

use crate::defs::RequestId;

use super::Handler;

/// A message to be sent to the background dedicated writer task.
pub(crate) enum DeferredDirective {
    /// Close the writer transport immediately after receiving this message.
    CloseImmediately,

    /// Close the writer transport after flushing all pending write requests.
    ///
    /// The rx channel, which is used to receive this message, will be closed right after this
    /// message is received.
    CloseAfterFlush,

    /// Flush the writer transport.
    Flush,

    /// Write a notification message.
    WriteNoti(Bytes),

    /// Write a request message. If the sending of the request is aborted by the writer, the
    /// request message will be revoked and will wake up the pending task.
    WriteReq(Bytes, RequestId),
}

/// Inbound message that was sent from the background dedicated reader task.
pub(crate) enum InboundMessageInner {
    // TODO: Notify, Request
}

// ========================================================== Service ===|

impl Handler {
    // TODO:
}
