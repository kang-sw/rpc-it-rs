use std::num::NonZeroU64;

use bytes::{Bytes, BytesMut};
use futures_util::future::join_all;

use crate::rpc::{SendError, Sender};

/// [`BroadcastProxy`] is responsible for broadcasting a single notification message to multiple
/// peers. It optimizes the encoding process by reusing encoded chunks when multiple peers share the
/// same codec for notification encoding.
#[derive(Clone)]
pub struct Broadcast {
    buffer: BytesMut,
    chunk_lut: Vec<(NonZeroU64, Bytes)>,
}

impl std::fmt::Debug for Broadcast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broadcast").field("chunk_lut", &self.chunk_lut).finish()
    }
}

impl Broadcast {
    /// Broadcast a notification to all connected clients in deferred manner.
    pub fn broadcast_deferred<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a Sender>,
        on_error: impl Fn(&'_ Sender, SendError),
    ) {
        for rpc in senders {
            let result = match self.lookup_chunk(rpc, method, params) {
                Ok(Some(bytes)) => rpc.write_bytes_deferred(bytes),
                Ok(None) => rpc.notify_deferred(method, params),
                Err(e) => Err(e),
            };

            if let Err(err) = result {
                on_error(rpc, err);
            }
        }

        self.chunk_lut.clear();
    }

    /// Broadcast a notification to all connected clients.
    pub async fn broadcast<'a, T: serde::Serialize>(
        &mut self,
        method: &str,
        params: &T,
        senders: impl IntoIterator<Item = &'a Sender>,
        on_error: impl Fn(&'_ Sender, SendError),
    ) {
        let iter = senders.into_iter();
        let size_hint = iter.size_hint();
        let mut futures = Vec::with_capacity(size_hint.1.unwrap_or(size_hint.0));
        let on_error = &on_error;

        for rpc in iter {
            let chunk = self.lookup_chunk(rpc, method, params);
            futures.push(async move {
                let result = match chunk {
                    Ok(Some(mut bytes)) => rpc.write_bytes(&mut bytes).await,
                    Ok(None) => rpc.notify(method, params).await,
                    Err(e) => Err(e),
                };

                if let Err(err) = result {
                    on_error(rpc, err);
                }
            });
        }

        join_all(futures).await;

        self.chunk_lut.clear();
    }

    fn lookup_chunk<T: serde::Serialize>(
        &mut self,
        rpc: &'_ Sender,
        method: &str,
        params: &T,
    ) -> Result<Option<Bytes>, SendError> {
        // Find chunk based on the codec's notification hash.
        let codec = rpc.codec();
        let Some(id) = codec.notification_encoder_hash() else {
            return Ok(None);
        };

        let index = match self.chunk_lut.binary_search_by_key(&id, |(id, ..)| *id) {
            Ok(index) => index,
            Err(insert_at) => {
                codec.encode_notify(method, params, &mut self.buffer)?;
                let bytes = self.buffer.split().freeze();
                self.chunk_lut.insert(insert_at, (id, bytes));
                insert_at
            }
        };

        Ok(Some(self.chunk_lut[index].1.clone()))
    }
}
