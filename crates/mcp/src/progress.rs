//! `notifications/progress` for a start: each `lane.phase` event the start's
//! own command streams becomes one notification.

use devlanes_proto::Event;
use rmcp::{
    model::{ProgressNotificationParam, ProgressToken},
    service::{Peer, RoleServer},
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

/// The progress message of a `lane.phase` event, such as `acme@1 infra
/// started`; `None` for other events.
pub fn message(event: &Event) -> Option<String> {
    if event.event != devlanes_proto::events::PHASE {
        return None;
    }
    let text = |key: &str| event.data.get(key).and_then(Value::as_str);
    Some(format!(
        "{} {} {}",
        text("lane")?,
        text("phase").unwrap_or("start"),
        text("state").unwrap_or("progress")
    ))
}

/// Sends each message of `messages` to `peer` under `token` until the channel
/// closes. `sent` counts over both runs of a call, so progress never goes
/// back. Without a token the messages are dropped.
pub async fn notify(
    mut messages: UnboundedReceiver<String>,
    token: Option<ProgressToken>,
    peer: Peer<RoleServer>,
    sent: &mut u32,
) {
    let Some(token) = token else {
        return;
    };
    while let Some(message) = messages.recv().await {
        *sent += 1;
        let param =
            ProgressNotificationParam::new(token.clone(), f64::from(*sent)).with_message(message);
        if peer.notify_progress(param).await.is_err() {
            break;
        }
    }
}
