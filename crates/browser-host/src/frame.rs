//! Chrome's native messaging framing: a 32-bit length in native byte order,
//! then that many bytes of UTF-8 JSON.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message the host reads from the extension. Requests are a few
/// hundred bytes, so anything bigger is a broken or hostile sender.
pub const MAX_IN: usize = 64 * 1024;

/// Largest message the host writes. Chrome refuses anything over 1 MiB from a
/// host and closes the port.
pub const MAX_OUT: usize = 1024 * 1024;

/// A frame that can't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("the extension sent a message of {0} bytes, over the {MAX_IN} byte limit")]
    TooLargeIn(usize),
    #[error("the message to the extension has {0} bytes, over the {MAX_OUT} byte limit")]
    TooLargeOut(usize),
    #[error("the extension closed the connection in the middle of a message")]
    Truncated,
    #[error("native messaging I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads one frame. `Ok(None)` is a clean end of input, before the first byte
/// of a length. A length over [`MAX_IN`] fails before its body is read.
pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut length = [0u8; 4];
    let mut filled = 0;
    // `read_exact` can't tell an end of input between frames from one inside a
    // length, and only the first one is a normal exit.
    while filled < length.len() {
        let read = reader.read(&mut length[filled..]).await?;
        if read == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
        filled += read;
    }
    let size = u32::from_ne_bytes(length) as usize;
    if size > MAX_IN {
        return Err(FrameError::TooLargeIn(size));
    }
    let mut body = vec![0u8; size];
    reader.read_exact(&mut body).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            FrameError::Truncated
        } else {
            FrameError::Io(error)
        }
    })?;
    Ok(Some(body))
}

/// Checks that `body` fits in one frame to the extension.
pub fn check_out(body: &[u8]) -> Result<(), FrameError> {
    if body.len() > MAX_OUT {
        return Err(FrameError::TooLargeOut(body.len()));
    }
    Ok(())
}

/// Writes one frame and flushes it, since Chrome reads each message as soon as
/// its bytes arrive.
pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> Result<(), FrameError> {
    check_out(body)?;
    // MAX_OUT fits in a u32, so the cast can't truncate.
    let length = (body.len() as u32).to_ne_bytes();
    writer.write_all(&length).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::ReadBuf;

    use super::*;

    /// A reader that hands out at most one byte per read, as a pipe may.
    struct Trickle(Vec<u8>, usize);

    impl AsyncRead for Trickle {
        /// Copies the next byte, or nothing at the end.
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(&byte) = self.0.get(self.1) {
                buf.put_slice(&[byte]);
                self.1 += 1;
            }
            Poll::Ready(Ok(()))
        }
    }

    /// `body` framed as Chrome frames it.
    fn framed(body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u32).to_ne_bytes().to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    #[tokio::test]
    async fn frames_arrive_whole_across_partial_reads() {
        let mut bytes = framed(br#"{"id":1}"#);
        bytes.extend(framed(b"{}"));
        let mut reader = Trickle(bytes, 0);
        assert_eq!(
            read(&mut reader).await.ok().flatten().as_deref(),
            Some(&br#"{"id":1}"#[..])
        );
        assert_eq!(
            read(&mut reader).await.ok().flatten().as_deref(),
            Some(&b"{}"[..])
        );
        assert!(matches!(read(&mut reader).await, Ok(None)));
    }

    #[tokio::test]
    async fn end_of_input_inside_a_frame_is_an_error() {
        let mut inside_length = Trickle(vec![3, 0], 0);
        assert!(matches!(
            read(&mut inside_length).await,
            Err(FrameError::Truncated)
        ));
        let mut bytes = framed(b"{}");
        bytes.pop();
        let mut inside_body = Trickle(bytes, 0);
        assert!(matches!(
            read(&mut inside_body).await,
            Err(FrameError::Truncated)
        ));
    }

    #[tokio::test]
    async fn an_oversized_request_fails_before_its_body() {
        let length = ((MAX_IN + 1) as u32).to_ne_bytes().to_vec();
        let mut reader = Trickle(length, 0);
        assert!(matches!(
            read(&mut reader).await,
            Err(FrameError::TooLargeIn(size)) if size == MAX_IN + 1
        ));
        let fits = framed(&vec![b' '; MAX_IN]);
        assert_eq!(
            read(&mut &fits[..])
                .await
                .ok()
                .flatten()
                .map(|body| body.len()),
            Some(MAX_IN)
        );
    }

    #[tokio::test]
    async fn an_oversized_reply_is_refused_and_nothing_is_written() {
        let mut written = Vec::new();
        let result = write(&mut written, &vec![b' '; MAX_OUT + 1]).await;
        assert!(matches!(result, Err(FrameError::TooLargeOut(_))));
        assert!(written.is_empty());
        assert!(write(&mut written, &vec![b' '; MAX_OUT]).await.is_ok());
        assert_eq!(written.len(), 4 + MAX_OUT);
        assert_eq!(&written[..4], &(MAX_OUT as u32).to_ne_bytes());
    }
}
