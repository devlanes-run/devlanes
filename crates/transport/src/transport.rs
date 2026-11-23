//! The connection abstraction every remote transport implements, and the
//! byte streams it carries.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Instant;

/// Boxed error used where the concrete cause belongs to an integration crate.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A bidirectional byte stream that can move between tasks.
pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

/// Type-erased duplex, such as one stream of a [`Connection`].
pub type BoxDuplex = Box<dyn Duplex>;

/// Type-erased client side of a connection, returned by [`Transport::connect`].
pub type BoxConnection = Box<dyn Connection>;

/// Type-erased box side of a connection, taken by [`crate::serve`] and [`crate::refuse`].
pub type BoxServerConnection = Box<dyn ServerConnection>;

/// The client's side of one connection to a box. Only the client opens streams.
///
/// A stream may reach the box only once the client wrote to it, so every
/// protocol stream starts with a client frame. Reads and writes on every
/// stream end once the connection closes or fails. A reset stream may read
/// as end-of-stream or as an error; the session ends on either.
#[async_trait]
pub trait Connection: Send + Sync {
    /// Opens a protocol stream. Shutting down its write half ends what the box
    /// reads and keeps the other direction; dropping it ends the stream, and the
    /// box still reads every byte written before.
    ///
    /// The session holds at most [`crate::MAX_STREAMS`] streams at once, and
    /// may drop one and open another at once, any number of times; `open` must
    /// not wait for that. The transport's own limit has to cover every stream
    /// the session dropped that the transport has not freed yet, so it needs
    /// headroom above [`crate::MAX_STREAMS`]. A transport that returns stream
    /// credit in batches, as QUIC does, needs more headroom than yamux's 64;
    /// iroh 1.5.0, for example, blocked with 64 and passed with 128.
    ///
    /// The one wait allowed is yamux's: it opens no stream while 256 it opened
    /// have had no reply from the box yet, so the box must answer streams.
    async fn open(&self) -> Result<BoxDuplex, ConnectionError>;

    /// When bytes last arrived from the box that show its process is running,
    /// on tokio's clock; the connection's start before anything arrived.
    ///
    /// Stream data counts, and so do replies the box's protocol stack sends
    /// to this side, such as yamux's pongs and window updates. Traffic that
    /// arrives whether or not the box's process runs, such as a QUIC
    /// transport's keep-alives or ACKs, must not count, or the session's ping
    /// timeout never notices a hung box. This side's own writes never count.
    fn last_inbound(&self) -> Instant;

    /// Waits up to `limit` for the reason the connection failed, such as the
    /// stderr of an SSH tunnel that exited. `None` when it closed cleanly, by
    /// either side (including after this side's own [`Connection::shutdown`]),
    /// or when no reason arrived in time. A close the transport starts itself,
    /// such as dropping a path it may not use or a box that breaks the
    /// protocol, is a failure and returns its reason.
    async fn failure_within(&self, limit: Duration) -> Option<String>;

    /// Starts closing the connection and its streams; returns at once.
    fn shutdown(&self);

    /// Resolves once the connection is closed by either side or has failed.
    async fn closed(&self);
}

/// The box's side of one connection: it accepts the streams the client opens.
///
/// Accepted streams behave like opened ones in both directions: shutting
/// down the write half keeps the other direction, and a dropped stream ends
/// after the bytes written before. Reads and writes on every stream end once
/// the connection closes or fails.
///
/// Dropping it closes the connection. One control message a stream wrote and
/// shut down before the drop, such as `Incompatible`, `PairingRefused` or
/// `Refused` (a few hundred bytes), still reaches the client while the
/// process keeps running. A transport whose close discards unsent stream
/// data, as QUIC's does, waits a bounded time for the client to close before
/// it closes on drop. Nothing larger is promised. When the box process exits
/// right after [`crate::serve`] returns, as `devlanes tunnel --stdio` does
/// after `Incompatible` and `PairingRefused`, that message is best effort; a
/// box that must be sure the client read its last words waits for the client
/// to close first, as [`crate::refuse`] does.
#[async_trait]
pub trait ServerConnection: Send {
    /// The next stream the client opened; `None` once the connection is gone.
    ///
    /// A stream the client opened and wrote to before opening any other is
    /// accepted first, which is how the box finds the control stream. Nothing
    /// else about the order of accepted streams is promised.
    async fn accept(&mut self) -> Option<BoxDuplex>;

    /// When bytes last arrived from the client that show its process is
    /// running, on tokio's clock; the connection's start before anything
    /// arrived. What counts is as for [`Connection::last_inbound`]: stream
    /// data and replies from the client's protocol stack, never keep-alives
    /// that arrive whether or not its process runs, and never this side's
    /// own writes.
    fn last_inbound(&self) -> Instant;
}

/// Failure to open a stream on a [`Connection`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectionError {
    /// The connection is closed; no further streams can be opened.
    #[error("mux connection closed")]
    Closed,
}

/// Produces one authenticated connection to a box per call.
///
/// Implementations do not interpret the bytes on its streams; the protocol
/// runs on top. A transport with one byte pipe to the box wraps it with
/// [`crate::mux_client`].
#[async_trait]
pub trait Transport: Send + Sync {
    /// Opens a fresh connection to the box.
    ///
    /// Return [`TransportError::Permanent`] when retrying cannot help (missing
    /// binary) so reconnect loops stop instead of spinning,
    /// [`TransportError::AuthFailed`] for rejected credentials on a box that
    /// never connected, and [`TransportError::NeedsAuth`] when a person has to
    /// sign in again to a box that worked before, so [`crate::Link`] waits for
    /// them.
    async fn connect(&self) -> Result<BoxConnection, TransportError>;
}

/// Failure to produce a connection, classified for reconnect decisions.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Local or network I/O failed; retrying later may succeed.
    #[error("transport I/O: {0}")]
    Io(#[from] io::Error),
    /// A failure that may clear up on its own, such as an unreachable host.
    #[error(transparent)]
    Transient(BoxError),
    /// A failure that needs user action before another attempt makes sense.
    #[error(transparent)]
    Permanent(BoxError),
    /// The box rejected the credentials or the host key on a box that never
    /// connected. Permanent like [`TransportError::Permanent`].
    #[error(transparent)]
    AuthFailed(BoxError),
    /// A person has to answer a prompt or fix credentials or a host key before
    /// another attempt makes sense, on a box that connected before.
    #[error(transparent)]
    NeedsAuth(BoxError),
}

impl TransportError {
    /// Wraps a cause that a later retry may not hit again.
    pub fn transient(error: impl Into<BoxError>) -> Self {
        Self::Transient(error.into())
    }

    /// Wraps a cause that requires user action, stopping reconnect loops.
    pub fn permanent(error: impl Into<BoxError>) -> Self {
        Self::Permanent(error.into())
    }

    /// Wraps rejected credentials or a rejected host key on a box that never
    /// connected, which stops reconnect loops like [`TransportError::permanent`].
    pub fn auth_failed(error: impl Into<BoxError>) -> Self {
        Self::AuthFailed(error.into())
    }

    /// Wraps a cause that a person resolves by signing in again, which makes
    /// [`crate::Link`] wait for [`crate::Link::resume`].
    pub fn needs_auth(error: impl Into<BoxError>) -> Self {
        Self::NeedsAuth(error.into())
    }

    /// An authentication failure reclassified as [`TransportError::NeedsAuth`],
    /// for a caller that knows a prompt went unanswered during the attempt.
    /// Other failures keep their class: the attempt got past authentication,
    /// so the missing answer is not what failed it.
    pub fn into_needs_auth(self) -> Self {
        match self {
            Self::AuthFailed(error) => Self::NeedsAuth(error),
            other => other,
        }
    }

    /// Reports whether reconnecting without user action is pointless.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_) | Self::AuthFailed(_))
    }

    /// Reports whether a person has to sign in again before reconnecting.
    pub fn is_needs_auth(&self) -> bool {
        matches!(self, Self::NeedsAuth(_))
    }

    /// Returns the integration-specific cause when it has type `E`.
    ///
    /// Callers use this to recover typed details such as an SSH failure.
    pub fn downcast_ref<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            Self::Io(_) => None,
            Self::Transient(error)
            | Self::Permanent(error)
            | Self::AuthFailed(error)
            | Self::NeedsAuth(error) => error.downcast_ref(),
        }
    }
}

/// Joins a separate reader and writer, such as child stdio, into one duplex.
#[derive(Debug)]
pub struct Joined<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> Joined<R, W> {
    /// Combines `reader` and `writer`; shutting down the duplex shuts down `writer`.
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    /// Returns the original halves.
    pub fn into_parts(self) -> (R, W) {
        (self.reader, self.writer)
    }
}

impl<R: AsyncRead + Unpin, W: Unpin> AsyncRead for Joined<R, W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(cx, buf)
    }
}

impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for Joined<R, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(cx)
    }
}

/// Exit status of `devlanes tunnel --stdio` when no box daemon runs and the
/// box's user config turns off starting one (`remote.start_daemon: false`).
/// Transports that run the tunnel report it as a temporary failure, so the
/// client keeps retrying until the daemon is started. 75 is `EX_TEMPFAIL`.
pub const EXIT_DAEMON_STOPPED: i32 = 75;

/// The process's stdin and stdout as one duplex, for `devlanes tunnel --stdio` on the box.
pub fn stdio() -> Joined<tokio::io::Stdin, tokio::io::Stdout> {
    Joined::new(tokio::io::stdin(), tokio::io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an authentication failure waits for a sign-in after an
    /// unanswered prompt; other failures keep their class.
    #[test]
    fn only_auth_failures_turn_into_needs_auth() {
        let rejected = TransportError::auth_failed("Permission denied (publickey).");
        assert!(rejected.is_permanent());
        assert!(rejected.into_needs_auth().is_needs_auth());

        let transient = TransportError::transient("connection reset").into_needs_auth();
        assert!(!transient.is_needs_auth() && !transient.is_permanent());
        let io = TransportError::from(io::Error::from(io::ErrorKind::BrokenPipe)).into_needs_auth();
        assert!(!io.is_needs_auth() && !io.is_permanent());
        let missing = TransportError::permanent("devlanes is not installed").into_needs_auth();
        assert!(missing.is_permanent() && !missing.is_needs_auth());
    }
}
