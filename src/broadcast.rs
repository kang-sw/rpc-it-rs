use crate::rpc;

/// Allows broadcast of single message across multiple clients.
///
///
pub struct BroadcastProxy {}

impl BroadcastProxy {
    /// Broadcast a notification to all connected clients in deferred manner.
    pub fn broadcast_deferred<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a rpc::Sender>,
        on_error: impl Fn(&'a rpc::Sender, rpc::SendError),
    ) {
        todo!()
    }

    /// Broadcast a notification to all connected clients.
    pub async fn broadcast<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a rpc::Sender>,
        on_error: impl Fn(&'a rpc::Sender, rpc::SendError),
    ) {
        todo!()
    }
}
