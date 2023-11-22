use bytes::Bytes;
use futures_util::future::join_all;

use crate::{codec::EncodeError, rpc};

/// [`BroadcastProxy`] is responsible for broadcasting a single notification message to multiple
/// peers. It optimizes the encoding process by reusing encoded chunks when multiple peers share the
/// same codec for notification encoding.
pub struct Broadcast {
    chunk_lut: Vec<rpc::EncodedNotify>,
}

impl Broadcast {
    /// Broadcast a notification to all connected clients in deferred manner.
    pub fn broadcast_deferred<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a rpc::Sender>,
        on_error: impl Fn(&'_ rpc::Sender, rpc::SendError),
    ) {
        self.chunk_lut.clear();

        for rpc in senders {
            if let Some(bytes) = self.lookup_chunk(rpc, method, params, &on_error) {
                if let Err(e) = rpc.write_bytes_deferred(bytes) {
                    on_error(rpc, e);
                }
            } else {
                if let Err(e) = rpc.notify_deferred(method, params) {
                    on_error(rpc, e);
                }
            }
        }
    }

    /// Broadcast a notification to all connected clients.
    pub async fn broadcast<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a rpc::Sender>,
        on_error: impl Fn(&'_ rpc::Sender, rpc::SendError),
    ) {
        self.chunk_lut.clear();

        let iter = senders.into_iter();
        let size_hint = iter.size_hint();
        let mut futures = Vec::with_capacity(size_hint.1.unwrap_or(size_hint.0));
        let on_error = &on_error;

        for rpc in iter {
            let chunk = self.lookup_chunk(rpc, method, params, on_error);
            futures.push(async move {
                if let Some(mut bytes) = chunk {
                    if let Err(e) = rpc.write_bytes(&mut bytes).await {
                        on_error(rpc, e);
                    }
                } else {
                    if let Err(e) = rpc.notify(method, params).await {
                        on_error(rpc, e);
                    }
                }
            });
        }

        join_all(futures).await;
    }

    fn lookup_chunk<T: serde::Serialize>(
        &mut self,
        sender: &'_ rpc::Sender,
        method: &str,
        params: &T,
        on_error: &impl Fn(&'_ rpc::Sender, rpc::SendError),
    ) -> Option<Bytes> {
        todo!()
    }
}
