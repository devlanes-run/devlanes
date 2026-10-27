//! The box registry, followed through the daemon's `remote.registry` feed.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use async_trait::async_trait;
use devlanes_client::{Client, Frame, Paths};
use devlanes_proto::{
    Context, Control, Event, Method, Outcome, RpcError,
    remote::{
        MirrorReport, REGISTRY_CHANGE_EVENT, REGISTRY_SNAPSHOT_EVENT, RegistryParams, RemoteClient,
        ReportParams,
    },
};
use devlanes_transport::{
    BoxError, ClientInfo, RegistryEvent, RegistryFeed, RegistrySnapshot, RegistrySource,
    RegistryUpdate,
};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::time::timeout;

/// Limit for connecting to the box daemon and for its answer to one report;
/// a report that takes longer is dropped, and the next change sends another.
const REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// [`RegistrySource`] over the box daemon's socket.
pub(crate) struct DaemonRegistry {
    paths: Paths,
    client: RemoteClient,
    /// The connection mirror reports go over, opened on the first one; the
    /// feed's own connection is busy streaming.
    reports: tokio::sync::Mutex<Option<Client>>,
    /// The serial of the `status` entry each client's feed got, by client id,
    /// which its reports name so they update that entry only. A tunnel
    /// serves one session; in-process tests serve several, and a client's
    /// newest session wins.
    serials: Mutex<HashMap<String, u64>>,
    /// Clients passed the pairing gate: the daemon refuses the feed of one
    /// without a pairing ([`RegistryParams::paired`]).
    paired: bool,
}

impl DaemonRegistry {
    /// Reads the daemon behind `paths`; `client` describes this tunnel to
    /// `status`, and `paired` says the box requires pairing.
    pub(crate) fn new(paths: Paths, client: RemoteClient, paired: bool) -> Self {
        Self {
            paths,
            client,
            reports: tokio::sync::Mutex::new(None),
            serials: Mutex::new(HashMap::new()),
            paired,
        }
    }

    /// Sends one `remote.report`, over the kept connection or a new one,
    /// within [`REPORT_TIMEOUT`] for the connect and for the answer.
    async fn send_report(&self, params: &serde_json::Value) -> Result<(), String> {
        let mut connection = self.reports.lock().await;
        if connection.is_none() {
            let connected = timeout(REPORT_TIMEOUT, Client::connect(&self.paths))
                .await
                .map_err(|_| "connecting to the box daemon timed out".to_owned())?
                .map_err(|error| error.to_string())?;
            *connection = Some(connected);
        }
        let Some(daemon) = connection.as_mut() else {
            return Ok(());
        };
        let sent = timeout(
            REPORT_TIMEOUT,
            daemon.request(Method::RemoteReport, params.clone(), Context::default()),
        )
        .await;
        match sent {
            Ok(Ok(_)) => Ok(()),
            // A broken or stuck connection is opened again for the next report.
            Ok(Err(error)) => {
                *connection = None;
                Err(error.to_string())
            }
            Err(_) => {
                *connection = None;
                Err("the box daemon did not answer the report in time".to_owned())
            }
        }
    }
}

/// Why the daemon's registry feed cannot be used.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FeedError {
    /// The connection to the box daemon failed.
    #[error("box daemon: {0}")]
    Daemon(#[from] devlanes_client::Error),
    /// The daemon predates `remote.registry`: it runs an older release than this tunnel.
    #[error(
        "the box daemon runs an older devlanes than `devlanes tunnel`; run `devlanes daemon \
         restart` on the box"
    )]
    OldDaemon,
    /// The daemon refused the request.
    #[error("the box daemon refused remote.registry: {} ({})", .0.code, .0.data)]
    Refused(RpcError),
    /// The daemon ended the feed.
    #[error("the box daemon ended the registry feed")]
    Ended,
    /// The tunnel could not encode its own `remote.registry` params.
    #[error("encoding the remote.registry request: {0}")]
    Params(serde_json::Error),
    /// An event did not have the shape of its name.
    #[error("the box daemon sent a malformed {event} event: {error}")]
    Malformed {
        /// The event name.
        event: String,
        /// What did not decode.
        error: serde_json::Error,
    },
}

/// Payload of [`REGISTRY_SNAPSHOT_EVENT`].
#[derive(Deserialize)]
struct SnapshotData {
    snapshot: RegistrySnapshot,
}

/// Payload of [`REGISTRY_CHANGE_EVENT`].
#[derive(Deserialize)]
struct ChangeData {
    change: RegistryEvent,
}

#[async_trait]
impl RegistrySource for DaemonRegistry {
    /// Starts `remote.registry` for `client` and waits for its first snapshot.
    async fn subscribe(&self, client: &ClientInfo) -> Result<RegistryFeed, BoxError> {
        let mut daemon = Client::connect(&self.paths)
            .await
            .map_err(FeedError::from)?;
        let params = RegistryParams {
            client: Some(RemoteClient {
                id: client.id.clone(),
                name: client.name.clone(),
                ..self.client.clone()
            }),
            features: client.features.clone(),
            paired: self.paired,
        };
        let params = serde_json::to_value(params).map_err(FeedError::Params)?;
        daemon
            .begin_request(Method::RemoteRegistry, params, Context::default())
            .await
            .map_err(FeedError::from)?;
        let snapshot = match next_update(&mut daemon).await? {
            (RegistryUpdate::Resync(snapshot), serial) => {
                if let Some(serial) = serial {
                    self.serials
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(client.id.clone(), serial);
                }
                snapshot
            }
            (RegistryUpdate::Event(_), _) => {
                return Err(FeedError::Malformed {
                    event: REGISTRY_CHANGE_EVENT.into(),
                    error: serde::de::Error::custom("a change came before the snapshot"),
                }
                .into());
            }
        };
        // Ends after the first error: the session ends with it.
        let updates = futures_util::stream::unfold(Some(daemon), |daemon| async move {
            let mut daemon = daemon?;
            match next_update(&mut daemon).await {
                Ok((update, _)) => Some((Ok(update), Some(daemon))),
                Err(error) => Some((Err(BoxError::from(error)), None)),
            }
        })
        .boxed();
        Ok(RegistryFeed { snapshot, updates })
    }

    /// Passes the report to the box daemon with `remote.report`, bounded, for
    /// the `status` entry the feed listed. A report that does not decode or
    /// does not arrive is dropped: the next change sends a fresh one, and
    /// `status` shows the last one that arrived meanwhile.
    async fn report(&self, client: &ClientInfo, report: serde_json::Value) {
        let report: MirrorReport = match serde_json::from_value(report) {
            Ok(report) => report,
            Err(error) => {
                tracing::debug!(%error, "dropping a malformed mirror report");
                return;
            }
        };
        let serial = self
            .serials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&client.id)
            .copied();
        // Until the snapshot names this tunnel's `status` entry, the report
        // has no entry to go to and the next change sends a fresh one.
        let Some(serial) = serial.filter(|_| !client.id.is_empty()) else {
            return;
        };
        let params = ReportParams {
            id: client.id.clone(),
            serial: Some(serial),
            report: report.bounded(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return;
        };
        if let Err(error) = self.send_report(&params).await {
            tracing::debug!(%error, "the box daemon did not take a mirror report");
        }
    }
}

/// The next registry update of the feed on `daemon`, with the serial of the
/// client's `status` entry that snapshots carry (`client_serial`). Other
/// events are skipped, so a newer daemon may add some.
async fn next_update(daemon: &mut Client) -> Result<(RegistryUpdate, Option<u64>), FeedError> {
    loop {
        match daemon.next_frame().await? {
            Frame::Control(Control::Event(event)) => {
                if let Some(update) = decode(&event)? {
                    let serial = event.data.get("client_serial").and_then(|v| v.as_u64());
                    return Ok((update, serial));
                }
            }
            Frame::Control(Control::Response(response)) => {
                return Err(match response.result {
                    Outcome::Error { error } if error.code == "unknown_method" => {
                        FeedError::OldDaemon
                    }
                    Outcome::Error { error } => FeedError::Refused(error),
                    Outcome::Ok { .. } => FeedError::Ended,
                });
            }
            _ => {}
        }
    }
}

/// The update a feed event carries; `None` for other events.
fn decode(event: &Event) -> Result<Option<RegistryUpdate>, FeedError> {
    let malformed = |error| FeedError::Malformed {
        event: event.event.clone(),
        error,
    };
    match event.event.as_str() {
        REGISTRY_SNAPSHOT_EVENT => {
            let data: SnapshotData =
                serde_json::from_value(event.data.clone()).map_err(malformed)?;
            Ok(Some(RegistryUpdate::Resync(data.snapshot)))
        }
        REGISTRY_CHANGE_EVENT => {
            let data: ChangeData = serde_json::from_value(event.data.clone()).map_err(malformed)?;
            Ok(Some(RegistryUpdate::Event(data.change)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use devlanes_transport::{LaneKey, UnitKey, UnitKind};
    use serde_json::json;

    use super::*;

    fn event(name: &str, data: serde_json::Value) -> Event {
        Event {
            seq: 0,
            event: name.into(),
            data,
        }
    }

    #[test]
    fn feed_events_decode_into_updates() -> Result<(), FeedError> {
        let snapshot = decode(&event(
            REGISTRY_SNAPSHOT_EVENT,
            json!({"snapshot": {"lanes": [], "units": []}}),
        ))?;
        assert!(matches!(snapshot, Some(RegistryUpdate::Resync(ref s)) if s.units.is_empty()));
        let change = decode(&event(
            REGISTRY_CHANGE_EVENT,
            json!({"change": {"type": "unit_removed", "unit": {
                "lane": {"project": "app", "lane": "0"}, "kind": "task", "name": "migrate"
            }}}),
        ))?;
        let removed = UnitKey {
            lane: LaneKey {
                project: "app".into(),
                lane: "0".into(),
            },
            kind: UnitKind::Task,
            name: "migrate".into(),
        };
        assert!(matches!(
            change,
            Some(RegistryUpdate::Event(RegistryEvent::UnitRemoved { ref unit })) if *unit == removed
        ));
        assert!(decode(&event("lane.changed", json!({})))?.is_none());
        Ok(())
    }

    /// A malformed answer is an error, never an empty registry that would
    /// unbind everything on the client.
    #[test]
    fn malformed_feed_events_are_errors() {
        for data in [
            json!({}),
            json!({"snapshot": {"lanes": "nope"}}),
            json!({"snapshot": null}),
        ] {
            let result = decode(&event(REGISTRY_SNAPSHOT_EVENT, data.clone()));
            assert!(
                matches!(result, Err(FeedError::Malformed { .. })),
                "{data}: {result:?}"
            );
        }
        let result = decode(&event(
            REGISTRY_CHANGE_EVENT,
            json!({"change": {"type": "x"}}),
        ));
        let message = result.expect_err("unknown change").to_string();
        assert!(message.contains("remote.registry.change"), "{message}");
    }

    /// A mirror report goes to the box daemon as `remote.report` with the
    /// client's id and its entry's serial, bounded; a malformed one, and one
    /// before the feed named the entry, are dropped without a request.
    #[tokio::test]
    async fn mirror_reports_go_to_the_daemon_bounded() -> crate::test_daemon::TestResult {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = crate::test_daemon::home(dir.path())?;
            let daemon = crate::test_daemon::answer_one(&paths, |_| Outcome::Ok {
                data: json!({"stored": true}),
            })
            .await?;
            let registry = DaemonRegistry::new(paths, RemoteClient::default(), false);
            let client = ClientInfo {
                id: "0123abcd".into(),
                ..ClientInfo::default()
            };
            registry
                .report(&client, json!({"ports": "not a list"}))
                .await;
            let ports: Vec<_> = (0..devlanes_proto::remote::MAX_REPORT_PORTS + 3)
                .map(|port| json!({"port": port, "lane": "app@0", "unit": "web", "state": "bound"}))
                .collect();
            let report = json!({"alias": "box\u{1b}", "ports": ports});
            // Without the serial of its feed's entry the report has no owner.
            registry.report(&client, report.clone()).await;
            registry
                .serials
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert("0123abcd".into(), 7);
            registry.report(&client, report).await;
            drop(registry);
            let request = daemon.await??;
            assert_eq!(request.method, Method::RemoteReport);
            let params: ReportParams = serde_json::from_value(request.params)?;
            assert_eq!(params.id, "0123abcd");
            assert_eq!(params.serial, Some(7));
            assert_eq!(params.report.alias, "box");
            assert_eq!(
                params.report.ports.len(),
                devlanes_proto::remote::MAX_REPORT_PORTS
            );
            Ok(())
        })
        .await?
    }

    /// A box daemon from before `remote.registry` answers `unknown_method`,
    /// which names the fix instead of the bare code. The request carries the
    /// client's identity and the session's features.
    #[tokio::test]
    async fn an_old_daemon_is_named_with_the_fix() -> crate::test_daemon::TestResult {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let dir = tempfile::tempdir()?;
            let paths = crate::test_daemon::home(dir.path())?;
            let daemon = crate::test_daemon::answer_one(&paths, |_| Outcome::Error {
                error: RpcError::new("unknown_method", json!({"method": "unknown"}), 2),
            })
            .await?;
            let registry = DaemonRegistry::new(paths, RemoteClient::default(), true);
            let client = ClientInfo {
                label: "devlanes test".into(),
                id: "0123abcd".into(),
                name: "laptop".into(),
                features: vec!["mirror_report".into()],
            };
            let Err(error) = registry.subscribe(&client).await else {
                panic!("an old daemon gave a feed");
            };
            assert!(
                matches!(
                    error.downcast_ref::<FeedError>(),
                    Some(FeedError::OldDaemon)
                ),
                "{error}"
            );
            assert!(
                error.to_string().contains("`devlanes daemon restart`"),
                "{error}"
            );
            let request = daemon.await??;
            assert_eq!(request.method, Method::RemoteRegistry);
            let params: RegistryParams = serde_json::from_value(request.params)?;
            let sent = params.client.ok_or("no client in the params")?;
            assert_eq!(
                (sent.id.as_str(), sent.name.as_str()),
                ("0123abcd", "laptop")
            );
            assert_eq!(params.features, ["mirror_report"]);
            assert!(params.paired, "a box that requires pairing says so");
            Ok(())
        })
        .await?
    }
}
