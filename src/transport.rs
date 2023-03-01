use std::{io::IoSlice, pin::Pin};

/* ---------------------------------------------------------------------------------------------- */
/*                                        TRAIT DEFINITIONS                                       */
/* ---------------------------------------------------------------------------------------------- */
///
/// A message writer trait, which is used to write message to the underlying transport
/// layer.
///
pub trait AsyncFrameWrite: Send + Sync + Unpin {
    /// Called before writing a message
    fn poll_start_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _frame_size: usize,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Called until flushing all message
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>>;

    /// Called after writing single message
    fn poll_end_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Flush the underlying transport layer.
    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Shutdown the underlying transport layer.
    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

///
/// A message reader trait. This is used to read message from the underlying transport layer.
///
pub trait AsyncFrameRead: Send + Sync + Unpin {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>>;
}

/* ---------------------------------------------------------------------------------------------- */
/*                                            UTILITIES                                           */
/* ---------------------------------------------------------------------------------------------- */
pub mod util {
    use std::{future::poll_fn, io::IoSlice, pin::Pin};

    use crate::{AsyncFrameRead, AsyncFrameWrite};

    pub async fn write_all_slices<const N: usize>(
        this: &mut dyn AsyncFrameWrite,
        bufs: &[&[u8]; N],
    ) -> std::io::Result<usize> {
        let mut bufs: [_; N] = std::array::from_fn(|i| std::io::IoSlice::new(bufs[i]));
        write_vectored_all(this, &mut bufs).await
    }

    pub async fn write_vectored_all(
        this: &mut dyn AsyncFrameWrite,
        mut bufs: &'_ mut [IoSlice<'_>],
    ) -> std::io::Result<usize> {
        let mut total_written = 0;
        let n_total = bufs.iter().map(|x| x.len()).sum();

        poll_fn(|cx| Pin::new(&mut *this).poll_start_write(cx, n_total)).await?;

        while bufs.is_empty() == false {
            let mut n = poll_fn(|cx| Pin::new(&mut *this).poll_write(cx, bufs)).await?;
            total_written += n;

            // HACK: following logic should be replaced with IoSlice::advance when it is stabilized.
            let mut nremv = 0;
            for buf in bufs.iter() {
                if buf.len() <= n {
                    n -= buf.len();
                    nremv += 1;
                } else {
                    break;
                }
            }

            bufs = &mut bufs[nremv..];

            unsafe {
                if n > 0 {
                    let buf = &mut bufs[0];
                    let src = std::slice::from_raw_parts(buf.as_ptr().add(n), buf.len() - n);
                    *buf = IoSlice::new(src)
                }
            }
        }

        poll_fn(|cx| Pin::new(&mut *this).poll_end_write(cx)).await?;
        Ok(total_written)
    }

    pub async fn read_all(
        this: &mut dyn AsyncFrameRead,
        mut buf: &'_ mut [u8],
    ) -> std::io::Result<usize> {
        let mut total_read = 0;
        let until = buf.len();

        while total_read != until {
            let n = poll_fn(|cx| Pin::new(&mut *this).poll_read(cx, buf)).await?;

            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF",
                ));
            }

            total_read += n;
            buf = &mut buf[n..];
        }

        Ok(total_read)
    }
}
