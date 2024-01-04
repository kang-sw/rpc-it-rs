//! # Builder for RPC connection
//!
//! This is highest level API that the user interact with very-first.

use std::num::NonZeroUsize;

use crate::{
    codec::Codec,
    io::{AsyncFrameRead, AsyncFrameWrite},
};

use super::UserData;

///
pub struct Builder<Wr, Rd, U, C> {
    writer: Wr,
    reader: Rd,
    user_data: U,
    codec: C,
    cfg: Config,
}

/// Non-generic configuration for [`Builder`].
#[derive(Default)]
struct Config {
    /// Channel capacity for deferred directive queue.
    writer_channel_capacity: usize,
}

pub fn create_builder() -> Builder<(), (), (), ()> {
    Builder {
        writer: (),
        reader: (),
        user_data: (),
        codec: (),
        cfg: Config::default(),
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C> {
    pub fn with_writer<Wr2>(self, writer: Wr2) -> Builder<Wr2, Rd, U, C>
    where
        Wr2: AsyncFrameWrite,
    {
        Builder {
            writer,
            reader: self.reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_reader<Rd2>(self, reader: Rd2) -> Builder<Wr, Rd2, U, C>
    where
        Rd2: AsyncFrameRead,
    {
        Builder {
            writer: self.writer,
            reader,
            user_data: self.user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_user_data<U2>(self, user_data: U2) -> Builder<Wr, Rd, U2, C> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data,
            codec: self.codec,
            cfg: self.cfg,
        }
    }

    pub fn with_codec<C2>(self, codec: C2) -> Builder<Wr, Rd, U, C2> {
        Builder {
            writer: self.writer,
            reader: self.reader,
            user_data: self.user_data,
            codec,
            cfg: self.cfg,
        }
    }

    pub fn with_write_channel_capacity(self, capacity: NonZeroUsize) -> Self {
        Builder {
            cfg: Config {
                writer_channel_capacity: capacity.get(),
                ..self.cfg
            },
            ..self
        }
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C>
where
    Wr: AsyncFrameWrite,
    Rd: AsyncFrameRead,
    U: UserData,
    C: Codec,
{
    /// Creates client.
    ///
    /// # Warning
    ///
    /// This method must be executed under tokio runtime activated, to spawn tasks for background
    /// runner.
    pub fn build(self) -> super::RequestSender<U> {
        todo!()
    }
}

impl<Wr, Rd, U, C> Builder<Wr, Rd, U, C>
where
    Wr: AsyncFrameWrite,
    U: UserData,
    C: Codec,
{
    /// Creates write-only client.
    ///
    /// # Warning
    ///
    /// This method must be executed under tokio runtime activated, to spawn tasks for background
    /// runner.
    pub fn build_write_only(self) -> super::NotifySender<U> {
        // Spawn writer task
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        todo!()
    }
}
