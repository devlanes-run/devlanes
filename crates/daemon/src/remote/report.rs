//! Mirror reports (the `mirror_report` feature). Client side: what this
//! machine mirrors of a box, sent after the mirror changed. Box side: the URLs
//! each reporting client opens a unit at, for `url` and `hosts`.

use devlanes_proto::remote::{MirrorReport, MirroredPort, RemoteClient, SkippedLane};
use serde_json::{Value, json};

use super::HostConn;
use crate::{engine::Inner, model::lock};

impl HostConn {
    /// What this machine mirrors of the box, as its report says it: every
    /// mirrored port with its state, the skipped lanes and hostnames, this
    /// machine's HTTPS port (`https_port`) and whether it serves the box's own
    /// (`serves_box_port`). Reasons are worded for the box, which shows them
    /// as this machine's.
    pub(super) fn mirror_report(
        &self,
        https_port: Option<u16>,
        serves_box_port: bool,
    ) -> MirrorReport {
        let view = lock(&self.view);
        let ports = view
            .ports
            .iter()
            .filter_map(|entry| {
                let port = u16::try_from(entry.get("port")?.as_u64()?).ok()?;
                let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
                let conflicts = entry
                    .get("conflicts")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                let addresses: Vec<&str> = entry
                    .get("addresses")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let address = ["127.0.0.1", "::1"]
                    .into_iter()
                    .find(|wanted| addresses.contains(wanted))
                    .or_else(|| addresses.first().copied());
                let bound = address.is_some();
                let first = |key: &str| {
                    conflicts
                        .iter()
                        .find_map(|conflict| conflict.get(key).and_then(Value::as_str))
                        .map(str::to_owned)
                };
                Some(MirroredPort {
                    port,
                    lane: text("lane").to_owned(),
                    unit: text("unit").to_owned(),
                    state: text("state").to_owned(),
                    address: address.map(str::to_owned),
                    holder: first("holder").or_else(|| first("local")),
                    message: (!bound).then(|| first("message")).flatten(),
                })
            })
            .collect();
        let skipped_lanes = view
            .skipped
            .iter()
            .map(|skipped| {
                let reason = match skipped.get("box").and_then(Value::as_str) {
                    Some(other) => {
                        format!("box {other}, connected there earlier, serves the same hostnames")
                    }
                    None => "a lane of that machine serves the same hostnames".to_owned(),
                };
                SkippedLane {
                    lane: skipped
                        .get("lane")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    reason,
                }
            })
            .collect();
        let skipped_hosts = view
            .refused_hosts
            .iter()
            .filter_map(|refused| refused.get("host").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        MirrorReport {
            alias: self.name.clone(),
            https_port,
            serves_box_port,
            ports,
            skipped_lanes,
            skipped_hosts,
        }
        .bounded()
    }
}

/// What a box unit is for its client URLs.
pub(crate) struct UnitTarget<'a> {
    /// Box lane reference, `project@index`.
    pub lane: &'a str,
    pub unit: &'a str,
    /// The hostname its URL uses, when it has one.
    pub host: Option<&'a str>,
    /// The port its URL uses without a hostname.
    pub port: Option<u16>,
    /// Hidden units are never mirrored.
    pub hidden: bool,
}

impl Inner {
    /// For each connected client that sent a mirror report, the URL it opens
    /// `target` at: `{client, id, alias, url, reachable, reason}`, where
    /// `alias` is the name the client connects to this box by. Clients
    /// without a report (older releases) are left out.
    pub(crate) fn client_urls(&self, target: &UnitTarget<'_>) -> Vec<Value> {
        let box_port = self.integ.routes.https_port();
        lock(&self.remote.clients)
            .values()
            .filter_map(|(_, client)| {
                let report = client.mirror.as_ref()?;
                Some(client_url(client, report, target, box_port))
            })
            .collect()
    }
}

/// The name a client goes by in output: its host name, else its id.
fn client_label(client: &RemoteClient) -> String {
    [&client.name, &client.id]
        .into_iter()
        .find(|text| !text.is_empty())
        .cloned()
        .unwrap_or_else(|| "a client".to_owned())
}

/// The URL `client` opens `target` at, from its `report`. A hostname is
/// reachable unless its lane or the name is skipped there, through the
/// client's proxy on the box's HTTPS port (`box_port`) when it serves that
/// one, else its own. A plain port is reachable once the client bound it.
fn client_url(
    client: &RemoteClient,
    report: &MirrorReport,
    target: &UnitTarget<'_>,
    box_port: Option<u16>,
) -> Value {
    let entry = |url: Option<String>, reason: Option<String>| {
        json!({
            "client": client_label(client),
            "id": client.id,
            "alias": report.alias,
            "url": url,
            "reachable": url.is_some(),
            "reason": reason,
        })
    };
    let unreachable = |reason: String| entry(None, Some(reason));
    if target.hidden {
        return unreachable("the unit is hidden, so it is never mirrored".into());
    }
    if let Some(skipped) = report.skipped_lanes.iter().find(|s| s.lane == target.lane) {
        return unreachable(format!(
            "the lane is not mirrored there: {}",
            skipped.reason
        ));
    }
    let url = match (target.host, target.port) {
        (Some(host), _) => {
            if report
                .skipped_hosts
                .iter()
                .any(|skipped| skipped.eq_ignore_ascii_case(host))
            {
                return unreachable(format!(
                    "{host} is not routed there: it is outside the names that machine routes"
                ));
            }
            let port = if report.serves_box_port {
                box_port
            } else {
                report.https_port
            };
            devlanes_resolve::https_url(host, port)
        }
        (None, Some(port)) => {
            let mirrored = report
                .ports
                .iter()
                .find(|mirrored| mirrored.port == port && mirrored.unit == target.unit);
            match mirrored {
                Some(MirroredPort {
                    address: Some(address),
                    ..
                }) if address.contains(':') => format!("http://[{address}]:{port}"),
                Some(MirroredPort {
                    address: Some(address),
                    ..
                }) => format!("http://{address}:{port}"),
                Some(mirrored) => return unreachable(port_reason(mirrored)),
                None => {
                    return unreachable(format!(
                        "port {port} is not mirrored there: it is mirrored while the unit runs"
                    ));
                }
            }
        }
        (None, None) => return unreachable("the unit has no port or hostname".into()),
    };
    entry(Some(url), None)
}

/// Why a mirrored port that is not bound cannot be opened on the client.
fn port_reason(port: &MirroredPort) -> String {
    let number = port.port;
    match (port.state.as_str(), port.holder.as_deref()) {
        ("in_use", Some(holder)) => format!("port {number} is in use there by {holder}"),
        ("in_use", None) => format!("port {number} is in use there"),
        ("local_lane", Some(holder)) => format!("port {number} belongs to {holder} there"),
        ("local_lane", None) => format!("port {number} belongs to a local lane there"),
        ("failed", _) => format!("port {number} could not be bound there"),
        _ => format!("port {number} is not bound there yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> RemoteClient {
        RemoteClient {
            id: "0123abcd".into(),
            name: "laptop".into(),
            ..RemoteClient::default()
        }
    }

    fn target(host: Option<&str>, port: Option<u16>) -> UnitTarget<'_> {
        UnitTarget {
            lane: "acme@1",
            unit: "web",
            host,
            port,
            hidden: false,
        }
    }

    fn mirrored(state: &str, address: Option<&str>, holder: Option<&str>) -> MirroredPort {
        MirroredPort {
            port: 3000,
            lane: "acme@1".into(),
            unit: "web".into(),
            state: state.into(),
            address: address.map(str::to_owned),
            holder: holder.map(str::to_owned),
            message: None,
        }
    }

    /// A hostname opens through the client's own HTTPS port, or the box's
    /// when the client serves that one too.
    #[test]
    fn hostnames_use_the_port_the_client_serves_them_on() {
        let mut report = MirrorReport {
            https_port: Some(443),
            ..MirrorReport::default()
        };
        let host = target(Some("web.acme-1.localhost"), Some(3000));
        let url = client_url(&client(), &report, &host, Some(8443));
        assert_eq!(url["url"], "https://web.acme-1.localhost");
        assert_eq!(url["reachable"], true);
        assert_eq!(url["client"], "laptop");
        report.serves_box_port = true;
        let url = client_url(&client(), &report, &host, Some(8443));
        assert_eq!(url["url"], "https://web.acme-1.localhost:8443");
        report.https_port = Some(9443);
        report.serves_box_port = false;
        let url = client_url(&client(), &report, &host, Some(8443));
        assert_eq!(url["url"], "https://web.acme-1.localhost:9443");
    }

    /// A skipped lane or hostname, a busy port, a port not mirrored yet and
    /// a hidden unit are unreachable with a reason; a bound port opens on its
    /// address.
    #[test]
    fn unreachable_units_say_why() {
        let raw = target(None, Some(3000));
        let report = |port: Option<MirroredPort>| MirrorReport {
            ports: port.into_iter().collect(),
            ..MirrorReport::default()
        };
        let bound = client_url(
            &client(),
            &report(Some(mirrored("bound", Some("127.0.0.1"), None))),
            &raw,
            None,
        );
        assert_eq!(bound["url"], "http://127.0.0.1:3000");
        let v6 = client_url(
            &client(),
            &report(Some(mirrored("bound", Some("::1"), None))),
            &raw,
            None,
        );
        assert_eq!(v6["url"], "http://[::1]:3000");
        let busy = client_url(
            &client(),
            &report(Some(mirrored("in_use", None, Some("nginx (pid 812)")))),
            &raw,
            None,
        );
        assert_eq!(busy["reachable"], false);
        assert_eq!(
            busy["reason"],
            "port 3000 is in use there by nginx (pid 812)"
        );
        let absent = client_url(&client(), &report(None), &raw, None);
        assert!(
            absent["reason"]
                .as_str()
                .is_some_and(|r| r.contains("not mirrored")),
            "{absent}"
        );
        let skipped = MirrorReport {
            skipped_lanes: vec![SkippedLane {
                lane: "acme@1".into(),
                reason: "a lane of that machine serves the same hostnames".into(),
            }],
            ..MirrorReport::default()
        };
        let routed = target(Some("web.acme-1.localhost"), None);
        let lane = client_url(&client(), &skipped, &routed, None);
        assert_eq!(lane["reachable"], false);
        assert!(
            lane["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("the lane is not mirrored")),
            "{lane}"
        );
        let refused = MirrorReport {
            skipped_hosts: vec!["api.example.com".into()],
            ..MirrorReport::default()
        };
        let foreign = target(Some("api.example.com"), None);
        assert_eq!(
            client_url(&client(), &refused, &foreign, None)["reachable"],
            false
        );
        let hidden = UnitTarget {
            hidden: true,
            ..target(None, Some(3000))
        };
        assert_eq!(
            client_url(&client(), &MirrorReport::default(), &hidden, None)["reachable"],
            false
        );
    }
}
