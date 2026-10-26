//! `infra.external`: servers devlanes does not run, reached on a loopback port
//! the config declares. Each lane lists every entry as a unit of kind
//! `external` with its own hostnames; the port is the same in every lane, kept
//! out of new port blocks, and an allocated port that equals one is a config
//! error.

use std::collections::BTreeSet;

use devlanes_config::{ExpressionContext, HostConfig, ProjectConfig};
use devlanes_proto::RpcError;
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    model::{ExternalInfo, ExternalRoute},
};

/// The entries of `config` for one lane, hostnames evaluated with `context`.
/// A hostname whose expression fails here (an `infra.*` value before the
/// lane's infra ran) is left out, as `infra.hosts` routes are. `https_port`
/// is the proxy's, for the URLs.
pub(crate) fn external_infos(
    config: &ProjectConfig,
    context: &ExpressionContext,
    https_port: Option<u16>,
) -> Vec<ExternalInfo> {
    let mut infos = Vec::new();
    for (name, entry) in &config.infra.external {
        // Validation refused a bad address when the config loaded.
        let Ok(port) = entry.port() else { continue };
        let mut routes = Vec::new();
        for host in entry.hosts.iter().flatten() {
            let (template, fallback, upstream) = match host {
                HostConfig::Name(name) => (name, None, false),
                HostConfig::Route(route) => (
                    &route.host,
                    route.fallback.as_ref(),
                    matches!(
                        route.host_header,
                        Some(devlanes_config::HostHeader::Upstream)
                    ),
                ),
            };
            let fallback = match fallback.map(|value| value.evaluate(context)) {
                Some(Ok(value)) => value,
                None => false,
                Some(Err(error)) => {
                    tracing::debug!(%name, message = %error.message, "external host fallback unresolved");
                    continue;
                }
            };
            match template.evaluate(context) {
                Ok(host) => routes.push(ExternalRoute {
                    url: (!host.contains('*'))
                        .then(|| devlanes_resolve::https_url(&host, https_port)),
                    host,
                    fallback,
                    host_header_upstream: upstream,
                }),
                Err(error) => {
                    tracing::debug!(%name, message = %error.message, "external host unresolved");
                }
            }
        }
        let url = if entry.is_ipv6() {
            format!("http://[::1]:{port}")
        } else {
            format!("http://127.0.0.1:{port}")
        };
        infos.push(ExternalInfo {
            name: name.clone(),
            port,
            url,
            hidden: entry.hidden.unwrap_or(false),
            routes,
        });
    }
    infos
}

/// The config error for the first `infra.external` entry of `config` whose
/// port a unit of lane `reference` was allocated: `taken` lists each unit's
/// config path (`services.api`, `infra.postgres`) with a port. `None` when
/// they are all apart.
pub(crate) fn collision(
    config: &ProjectConfig,
    reference: &str,
    taken: impl IntoIterator<Item = (String, u16)>,
) -> Option<RpcError> {
    let taken: Vec<(String, u16)> = taken.into_iter().collect();
    for (name, entry) in &config.infra.external {
        let Ok(port) = entry.port() else { continue };
        if let Some((unit, _)) = taken.iter().find(|(_, used)| *used == port) {
            let path = format!("infra.external.{name}.address");
            let location = config.sources.position(&path).unwrap_or_default();
            return Some(RpcError::new(
                "config_error",
                json!({
                    "path": path,
                    "message": format!(
                        "{path}: port {port} is also {unit}'s port in {reference}; declare a port outside the range devlanes allocates from (`ports.range` in the user config)"
                    ),
                    "file": location.file,
                    "line": location.line,
                    "column": location.column,
                }),
                1,
            ));
        }
    }
    None
}

/// Every port a lane's infra namespace (`{name: {port, ports}}`) publishes,
/// with the container's config path.
pub(crate) fn infra_ports(infra: &Value) -> Vec<(String, u16)> {
    let Some(namespace) = infra.as_object() else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for (name, entry) in namespace {
        let numbers = entry.get("port").into_iter().chain(
            entry
                .get("ports")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|ports| ports.values()),
        );
        for port in numbers.filter_map(|value| u16::try_from(value.as_u64()?).ok()) {
            ports.push((format!("infra.{name}"), port));
        }
    }
    ports
}

impl Inner {
    /// The ports every loaded project declares in `infra.external`, which new
    /// port blocks keep clear of.
    pub(crate) fn external_ports(&self) -> BTreeSet<u16> {
        self.all_lanes()
            .iter()
            .filter_map(|(_, lane)| lane.st().config.loaded().cloned())
            .flat_map(|config| {
                config
                    .infra
                    .external
                    .values()
                    .filter_map(|entry| entry.port().ok())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with `pgadmin` on 5050 and a hidden `docs` on `[::1]:8000`
    /// whose hostname needs the `db` container's port.
    fn config() -> ProjectConfig {
        devlanes_config::parse_project(
            "project: acme\ninfra:\n  containers:\n    db: { image: postgres, ports: { pg: 5432 } }\n  external:\n    pgadmin:\n      address: 127.0.0.1:5050\n      hosts:\n        - pgadmin.${{ lane.domain }}\n        - { host: '*.pgadmin.${{ lane.domain }}', fallback: true }\n    docs:\n      address: '[::1]:8000'\n      hidden: true\n      hosts: docs.${{ infra.db.port }}.localhost\n",
            None,
        )
        .expect("parses")
    }

    /// Hostnames are evaluated for the lane, wildcards get no URL, a host
    /// whose expression fails is left out, and the address picks the
    /// loopback URL.
    #[test]
    fn entries_are_laid_out_per_lane() {
        let context = ExpressionContext {
            lane: json!({"domain": "acme-1.localhost"}),
            ..ExpressionContext::default()
        };
        let infos = external_infos(&config(), &context, Some(8443));
        assert_eq!(infos.len(), 2);
        let pgadmin = &infos[0];
        assert_eq!(pgadmin.port, 5050);
        assert_eq!(pgadmin.url, "http://127.0.0.1:5050");
        assert_eq!(pgadmin.routes.len(), 2);
        assert_eq!(pgadmin.routes[0].host, "pgadmin.acme-1.localhost");
        assert_eq!(
            pgadmin.public_url().as_deref(),
            Some("https://pgadmin.acme-1.localhost:8443")
        );
        assert!(pgadmin.routes[1].fallback && pgadmin.routes[1].url.is_none());
        let docs = &infos[1];
        assert!(docs.hidden);
        assert_eq!(docs.url, "http://[::1]:8000");
        assert!(docs.routes.is_empty(), "infra.db is not known yet");
    }

    /// An allocated port equal to an external one is a config error naming
    /// the entry and the unit; ports apart are none.
    #[test]
    fn allocated_ports_must_stay_apart() {
        let config = config();
        assert!(collision(&config, "acme@1", [("services.api".into(), 20000)]).is_none());
        let error = collision(
            &config,
            "acme@1",
            [("services.api".into(), 20000), ("infra.db".into(), 5050)],
        )
        .expect("collides");
        assert_eq!(error.code, "config_error");
        let message = error.data["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(
                "infra.external.pgadmin.address: port 5050 is also infra.db's port in acme@1"
            ),
            "{message}"
        );
        assert_eq!(
            infra_ports(&json!({"db": {"port": 5050, "ports": {"5432": 5050, "x": "y"}}})),
            [("infra.db".to_owned(), 5050), ("infra.db".to_owned(), 5050)]
        );
    }
}
