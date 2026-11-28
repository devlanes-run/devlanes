//! The pairing exchange over [`crate::PAIR_ALPN`]: a client that holds a
//! box's [`crate::Ticket`] sends one [`PairRequest`] on one stream, and the
//! box answers with one [`PairAnswer`]. The box admits any key on this ALPN
//! while a ticket is outstanding, so it gives such a peer one small, bounded
//! request and nothing else; sessions use [`crate::TUNNEL_ALPN`], which only
//! pinned keys reach.
//!
//! The order is chosen so the answer is not lost on QUIC, whose close drops
//! data the other end has not received: the client writes its request and
//! shuts down its write half; the box answers, shuts down its write half
//! and waits for the client to close; the client reads to the end and then
//! closes the connection.

use std::time::Duration;

use async_trait::async_trait;
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tokio_util::codec::Framed;

use crate::peer::TicketSecret;
use crate::transport::{BoxConnection, BoxServerConnection, ConnectionError};
use crate::wire::{self, FrameCodec, Secret, WireError};

/// Largest pairing frame either side accepts: the request comes from a key
/// nobody vouched for yet.
pub const PAIR_FRAME_LIMIT: usize = 4 * 1024;

/// How long the box waits for the first stream and then for the request on
/// it. Any key may open a pairing connection while a ticket is out, and a
/// client sends its request as soon as the connection is up, so an idle one
/// holds a pairing slot for twice this at most.
const REQUEST_STEP: Duration = Duration::from_secs(2);

/// How long the box waits to write its answer and then for the client to
/// close, which covers the answer's way to the client and back.
const ANSWER_STEP: Duration = Duration::from_secs(5);

/// How long the client waits for the box's answer and the end of its
/// stream: the box's own steps, plus storing the pairing.
const CLIENT_WAIT: Duration = Duration::from_secs(30);

/// The client's only message on a pairing stream. `Debug` leaves the secret
/// out.
#[derive(Serialize, Deserialize)]
pub struct PairRequest {
    /// The ticket's secret as 32 lowercase hex digits.
    secret: Secret,
    /// The client's `client-id` (`dl_cid_` and 64 hex digits).
    pub client_id: String,
    /// The client machine's host name.
    pub client_name: String,
    /// The client's version label, such as `devlanes 0.16.0`.
    pub client: String,
}

impl PairRequest {
    /// A request that presents `secret` for the client with this id, host
    /// name and version label.
    pub fn new(
        secret: &TicketSecret,
        client_id: String,
        client_name: String,
        client: String,
    ) -> Self {
        let hex = secret.0.iter().map(|byte| format!("{byte:02x}")).collect();
        Self {
            secret: Secret(hex),
            client_id,
            client_name,
            client,
        }
    }

    /// The secret the client presented, or `None` when it is not 32 hex
    /// digits, which the box counts as a wrong secret.
    pub fn secret(&self) -> Option<TicketSecret> {
        let digits = self.secret.0.as_bytes();
        if digits.len() != 32 {
            return None;
        }
        let digit = |byte: u8| char::from(byte).to_digit(16);
        let mut secret = [0u8; 16];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, [high, low]) in secret.iter_mut().zip(pairs) {
            let value = digit(*high)? << 4 | digit(*low)?;
            *byte = u8::try_from(value).ok()?;
        }
        Some(TicketSecret(secret))
    }
}

impl std::fmt::Debug for PairRequest {
    /// Everything but the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairRequest")
            .field("client_id", &self.client_id)
            .field("client_name", &self.client_name)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

/// The box's only message on a pairing stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairAnswer {
    /// The client's key is pinned; `name` is the box's host name and
    /// `server` its version label.
    Paired {
        /// The box's host name.
        name: String,
        /// The box's version label, such as `devlanes 0.16.0`.
        server: String,
    },
    /// The secret was wrong, expired or used, or pairing is paused.
    Refused {
        /// What the client shows its user.
        message: String,
        /// How long the box refuses every secret, after too many wrong ones.
        retry_after_ms: Option<u64>,
    },
}

/// Box side: decides a pairing request. The implementation knows which key
/// the connection came from.
#[async_trait]
pub trait PairDesk: Send + Sync {
    /// The answer to `request`; pins the client's key when it pairs.
    /// [`answer_pairing`] does not bound it, so the implementation bounds
    /// what it waits for before it uses the ticket.
    async fn decide(&self, request: &PairRequest) -> PairAnswer;
}

/// Why a pairing exchange failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PairError {
    /// The client could not open the pairing stream.
    #[error("opening the pairing stream: {0}")]
    Open(#[from] ConnectionError),
    /// A message could not be read or written.
    #[error("pairing: {0}")]
    Wire(#[from] WireError),
    /// The other side took too long for a step.
    #[error("the other side did not finish pairing in time")]
    Timeout,
    /// The other side closed before it sent its message.
    #[error("the other side closed the pairing connection before sending anything")]
    Closed,
}

/// Client side: sends `request` on a new stream of `connection`, a
/// connection to the box over [`crate::PAIR_ALPN`], and returns the box's
/// answer. Closes the connection once the box finished its side.
pub async fn request_pairing(
    connection: BoxConnection,
    request: PairRequest,
) -> Result<PairAnswer, PairError> {
    let result = timeout(CLIENT_WAIT, async {
        let stream = connection.open().await?;
        let mut channel = Framed::new(stream, FrameCodec::with_max(PAIR_FRAME_LIMIT));
        wire::send(&mut channel, &request).await?;
        // Shuts down the write half: the box reads to the end of the request.
        channel.close().await?;
        let (answer, _) = wire::recv::<_, PairAnswer>(&mut channel)
            .await?
            .ok_or(PairError::Closed)?;
        // The box shut down its side after the answer; anything more is ignored.
        while wire::recv::<_, serde_json::Value>(&mut channel)
            .await
            .is_ok_and(|frame| frame.is_some())
        {}
        Ok(answer)
    })
    .await
    .unwrap_or(Err(PairError::Timeout));
    // The client closes first, so the box's answer was delivered before it.
    connection.shutdown();
    result
}

/// Box side: reads one request from the first stream `connection` opens,
/// answers it with `desk`, then waits a bounded time for the client to close
/// before dropping the connection. Streams after the first are dropped
/// unread.
pub async fn answer_pairing(
    mut connection: BoxServerConnection,
    desk: &dyn PairDesk,
) -> Result<(), PairError> {
    let stream = timeout(REQUEST_STEP, connection.accept())
        .await
        .map_err(|_| PairError::Timeout)?
        .ok_or(PairError::Closed)?;
    let mut channel = Framed::new(stream, FrameCodec::with_max(PAIR_FRAME_LIMIT));
    let (request, _) = timeout(REQUEST_STEP, wire::recv::<_, PairRequest>(&mut channel))
        .await
        .map_err(|_| PairError::Timeout)??
        .ok_or(PairError::Closed)?;
    // Not bounded here: the desk bounds what it does before it uses the
    // ticket and must not be cut off after.
    let answer = desk.decide(&request).await;
    timeout(ANSWER_STEP, async {
        wire::send(&mut channel, &answer).await?;
        channel.close().await
    })
    .await
    .map_err(|_| PairError::Timeout)??;
    // Dropping the connection before the client closed could discard the
    // answer, so the client closes first; one that never does is dropped.
    let _ = timeout(ANSWER_STEP, async {
        while connection.accept().await.is_some() {}
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secret goes over the wire as hex and reads back as itself; text
    /// that is not 32 hex digits is no secret, and `Debug` never shows it.
    #[test]
    fn a_request_carries_its_secret_and_hides_it() -> Result<(), serde_json::Error> {
        let secret = TicketSecret(std::array::from_fn(|index| (index * 17) as u8));
        let request = PairRequest::new(
            &secret,
            "dl_cid_x".into(),
            "laptop".into(),
            "devlanes 0".into(),
        );
        let sent: PairRequest = serde_json::from_str(&serde_json::to_string(&request)?)?;
        assert_eq!(sent.secret(), Some(secret));
        assert!(!format!("{sent:?}").contains("1122"), "{sent:?}");
        for wrong in [
            "",
            "00",
            &"g".repeat(32),
            &"0".repeat(33),
            &format!("+{}", "0".repeat(31)),
        ] {
            let request: PairRequest = serde_json::from_value(serde_json::json!({
                "secret": wrong, "client_id": "", "client_name": "", "client": "",
            }))?;
            assert_eq!(request.secret(), None, "{wrong}");
        }
        Ok(())
    }
}
