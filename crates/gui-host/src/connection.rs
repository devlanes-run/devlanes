//! One connection: the hello and its checks, then a reader that forwards
//! allowed messages to the host task and a writer that drains the outbox.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::BytesMut;
use devlanes_gui_codec::select;
use devlanes_gui_proto::{
    ClientMessage, CodecOffer, DecodeError, Hello, HostMessage, PROTOCOL_VERSION, Refusal,
    RefusalReason, Role, SessionId, Token,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::HELLO_TIMEOUT;
use crate::hub::Inbox;
use crate::outbox::Outbox;

/// How long a refusal may take to write.
const REFUSAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// What every connection needs to know from the options.
#[derive(Debug)]
pub(crate) struct Settings {
    /// The control token, if the control role is served.
    pub(crate) token: Option<Token>,
    /// The encoders the host offers.
    pub(crate) encoders: Vec<CodecOffer>,
    /// The write queue cap.
    pub(crate) write_queue_cap: usize,
    /// One permit per welcomed connection.
    pub(crate) welcomed: Arc<Semaphore>,
}

/// Why the hello could not be read.
#[derive(Debug)]
enum HelloError {
    /// The peer closed or the read failed.
    Io,
    /// The bytes are not a hello of this version.
    Decode(DecodeError),
}

/// The connections still saying hello, oldest first. One more past the cap
/// closes the oldest, so peers that connect and say nothing cannot keep
/// new connections, the daemon's included, out.
#[derive(Debug)]
pub(crate) struct Hellos {
    queue: Mutex<VecDeque<(u64, CancellationToken)>>,
    next: AtomicU64,
    cap: usize,
}

impl Hellos {
    /// An empty queue of at most `cap` connections.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            next: AtomicU64::new(0),
            cap,
        }
    }

    /// The queue; a panic while it was held cannot leave it inconsistent.
    fn queue(&self) -> MutexGuard<'_, VecDeque<(u64, CancellationToken)>> {
        self.queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// A place for a new connection, closing the oldest one at the cap.
    pub(crate) fn admit(self: &Arc<Self>) -> PendingHello {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        let mut queue = self.queue();
        if queue.len() >= self.cap
            && let Some((_, oldest)) = queue.pop_front()
        {
            tracing::debug!("closing the oldest GUI connection still without a hello");
            oldest.cancel();
        }
        queue.push_back((id, token.clone()));
        PendingHello {
            hellos: self.clone(),
            id,
            token,
        }
    }
}

/// One connection's place among those still saying hello; dropping it
/// gives the place up.
#[derive(Debug)]
pub(crate) struct PendingHello {
    hellos: Arc<Hellos>,
    id: u64,
    token: CancellationToken,
}

impl Drop for PendingHello {
    /// Leaves the queue.
    fn drop(&mut self) {
        self.hellos.queue().retain(|(id, _)| *id != self.id);
    }
}

/// Runs one connection to its end. `pending` is its place among the
/// connections still saying hello, given up once the hello is read; a newer
/// connection may take it first, which closes this one.
pub(crate) async fn run<S>(
    stream: S,
    pending: PendingHello,
    inbox: mpsc::Sender<Inbox>,
    settings: Arc<Settings>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut buf = BytesMut::with_capacity(16 * 1024);
    let hello = tokio::select! {
        hello = tokio::time::timeout(HELLO_TIMEOUT, read_hello(&mut reader, &mut buf)) => hello,
        _ = pending.token.cancelled() => return,
    };
    drop(pending);
    let hello = match hello {
        Ok(Ok(hello)) => hello,
        Ok(Err(HelloError::Decode(DecodeError::VersionMismatch { theirs, .. }))) => {
            let message = format!(
                "this GUI host speaks protocol version {PROTOCOL_VERSION}, the client {theirs}"
            );
            refuse(&mut writer, RefusalReason::VersionMismatch, message).await;
            return;
        }
        Ok(Err(HelloError::Decode(error))) => {
            // Not this protocol, or garbage: there is nobody to answer.
            tracing::debug!(%error, "closing a GUI connection without a valid hello");
            return;
        }
        Ok(Err(HelloError::Io)) | Err(_) => return,
    };
    let codec = match check(&hello, &settings) {
        Ok(codec) => codec,
        Err((reason, message)) => {
            refuse(&mut writer, reason, message.to_owned()).await;
            return;
        }
    };
    // Held for the rest of the connection.
    let Ok(_place) = settings.welcomed.clone().try_acquire_owned() else {
        tracing::debug!("refusing a GUI connection past the limit");
        let message = "too many GUI connections".to_owned();
        refuse(&mut writer, RefusalReason::Other, message).await;
        return;
    };
    let role = hello.role;
    let outbox = Arc::new(Outbox::new(settings.write_queue_cap));
    let (reply_tx, reply_rx) = oneshot::channel();
    let join = Inbox::Join {
        hello,
        codec,
        outbox: outbox.clone(),
        reply: reply_tx,
    };
    if inbox.send(join).await.is_err() {
        return;
    }
    let Ok(session) = reply_rx.await else {
        return;
    };
    tokio::select! {
        _ = write_loop(&mut writer, &outbox) => {}
        _ = read_loop(&mut reader, buf, role, session, &inbox) => {}
        // Past the write cap: drop the stream now, even mid-write.
        _ = outbox.discarded() => {}
    }
    outbox.close(false);
    // Fails only once the host task ended, which forgets every session.
    let _ = inbox.send(Inbox::Leave { session }).await;
}

/// Checks the role, the token and, for a viewer, that both ends share a
/// codec. Returns the viewer's codec, or the refusal.
fn check(
    hello: &Hello,
    settings: &Settings,
) -> Result<Option<CodecOffer>, (RefusalReason, &'static str)> {
    match hello.role {
        Role::Viewer => match select(&settings.encoders, &hello.decoders) {
            Some(chosen) => Ok(Some(CodecOffer {
                codec: chosen.codec,
                hardware: chosen.encoder_hardware,
            })),
            None => Err((
                RefusalReason::Other,
                "the viewer decodes no codec this host encodes",
            )),
        },
        Role::Tool => Ok(None),
        Role::Control => match (&settings.token, &hello.token) {
            (None, _) => Err((
                RefusalReason::RoleNotSupported,
                "this GUI host has no control connection",
            )),
            (Some(ours), Some(theirs)) if ours == theirs => Ok(None),
            (Some(_), _) => Err((RefusalReason::BadToken, "the control token is wrong")),
        },
        Role::Input | Role::Unknown(_) => Err((
            RefusalReason::RoleNotSupported,
            "this GUI host does not serve that role",
        )),
    }
}

/// Reads until the hello is complete.
async fn read_hello<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut BytesMut,
) -> Result<Hello, HelloError> {
    loop {
        if let Some(hello) = ClientMessage::decode_hello(buf).map_err(HelloError::Decode)? {
            return Ok(hello);
        }
        match reader.read_buf(buf).await {
            Ok(0) | Err(_) => return Err(HelloError::Io),
            Ok(_) => {}
        }
    }
}

/// Sends a refusal and closes the write side, giving up after
/// [`REFUSAL_TIMEOUT`] on a peer that does not read.
async fn refuse<W: AsyncWrite + Unpin>(writer: &mut W, reason: RefusalReason, message: String) {
    let mut out = BytesMut::new();
    let refusal = HostMessage::Refused(Refusal { reason, message });
    let send = async {
        if refusal.encode(&mut out).is_ok() {
            let _ = writer.write_all(&out).await;
        }
        let _ = writer.shutdown().await;
    };
    let _ = tokio::time::timeout(REFUSAL_TIMEOUT, send).await;
}

/// Forwards each message the role may send; returns on the first one it may
/// not, on a decode error, at the end of the stream, or when the host task
/// is gone.
async fn read_loop<R: AsyncRead + Unpin>(
    reader: &mut R,
    mut buf: BytesMut,
    role: Role,
    session: SessionId,
    inbox: &mpsc::Sender<Inbox>,
) {
    loop {
        loop {
            match ClientMessage::decode(&mut buf) {
                Ok(Some(message)) if message.allowed_for(role) => {
                    if inbox
                        .send(Inbox::Message { session, message })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                // The message itself is not logged: it may carry clipboard text.
                Ok(Some(_)) => {
                    tracing::debug!(
                        ?role,
                        "closing a GUI connection that sent a message its role may not"
                    );
                    return;
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(%error, "closing a GUI connection that sent an invalid message");
                    return;
                }
            }
        }
        match reader.read_buf(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// Writes what the outbox holds until it is closed and empty or a write fails.
async fn write_loop<W: AsyncWrite + Unpin>(writer: &mut W, outbox: &Outbox) {
    while let Some(message) = outbox.pop().await {
        if writer.write_all(&message).await.is_err() {
            return;
        }
    }
    let _ = writer.shutdown().await;
}
