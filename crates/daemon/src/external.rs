//! `infra.external`: servers devlanes does not run, reached on a loopback port
//! the config declares. Each lane lists every entry as a unit of kind
//! `external` with its own hostnames; the port is the same in every lane and
//! goes to no unit, and one devlanes or any lane already uses is a config
//! error.

use std::collections::BTreeSet;

use devlanes_config::{ExpressionContext, ExternalHost, ProjectConfig};
use devlanes_proto::RpcError;
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    model::{ExternalInfo, ExternalRoute, LaneRt, ProjectRt},
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
        let Ok(address) = entry.socket_address() else {
            continue;
        };
        let port = address.port();
        let mut routes = Vec::new();
        for host in entry.hosts.iter().flat_map(|hosts| hosts.iter()) {
            let (template, fallback, upstream) = match host {
                ExternalHost::Name(name) => (name, None, false),
                ExternalHost::Route(route) => (
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
        infos.push(ExternalInfo {
            name: name.clone(),
            address,
            url: format!("http://{address}"),
            port,
            hidden: entry.hidden.unwrap_or(false),
            routes,
        });
    }
    infos
}

/// A port something other than an `infra.external` entry holds on this
/// machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortUse {
    pub port: u16,
    /// What holds it, worded to follow "port N is": `services.api's port in
    /// acme@1`, or `the devlanes proxy's HTTPS port`.
    pub holder: String,
    /// devlanes itself listens on it, so there is nothing to move.
    pub daemon: bool,
}

impl PortUse {
    /// `path`'s port in `lane`, such as `services.api` in `acme@1`.
    pub(crate) fn unit(port: u16, path: &str, lane: &str) -> Self {
        Self {
            port,
            holder: format!("{path}'s port in {lane}"),
            daemon: false,
        }
    }
}

/// The config error for the first `infra.external` entry of `config` whose
/// port is in `taken`; `None` when they are all apart. `pool` is the
/// inclusive `ports.range`, whose ports the advice says to leave to devlanes.
pub(crate) fn collision(
    config: &ProjectConfig,
    taken: &[PortUse],
    pool: Option<(u16, u16)>,
) -> Option<RpcError> {
    for (name, entry) in &config.infra.external {
        let Ok(port) = entry.port() else { continue };
        let Some(used) = taken.iter().find(|used| used.port == port) else {
            continue;
        };
        let path = format!("infra.external.{name}.address");
        let advice = match pool {
            _ if used.daemon => String::new(),
            Some((first, last)) if (first..=last).contains(&port) => format!(
                ". Pick a port outside {first}-{last}, the ports.range devlanes allocates from"
            ),
            // A literal `PORT` or a port a Compose file publishes as written.
            _ => ". Pick another port, or change the one that unit uses".to_owned(),
        };
        let location = config.sources.position(&path).unwrap_or_default();
        return Some(RpcError::new(
            "config_error",
            json!({
                "path": path,
                "message": format!("{path}: port {port} is {}{advice}", used.holder),
                "file": location.file,
                "line": location.line,
                "column": location.column,
            }),
            1,
        ));
    }
    None
}

/// The ports lane `reference` plans for its services (`(name, port, named
/// ports)`) and publishes for its containers (`infra`, its infra namespace).
pub(crate) fn lane_ports<'a>(
    reference: &str,
    services: impl IntoIterator<Item = (&'a str, u16, &'a std::collections::BTreeMap<String, u16>)>,
    infra: &Value,
) -> Vec<PortUse> {
    let mut ports = Vec::new();
    for (name, port, named) in services {
        let path = format!("services.{name}");
        for port in std::iter::once(port).chain(named.values().copied()) {
            ports.push(PortUse::unit(port, &path, reference));
        }
    }
    for (path, port) in infra_ports(infra) {
        ports.push(PortUse::unit(port, &path, reference));
    }
    ports
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
    /// The config error for an `infra.external` entry of lane `lane`'s
    /// `config` whose port is already in use by devlanes: the proxy's and the
    /// DNS server's ports, `own` (the lane's units and containers, as just
    /// planned), every lane's port block, and what every other lane of this
    /// machine plans or publishes, other projects' included.
    pub(crate) fn external_conflict(
        &self,
        config: &ProjectConfig,
        lane: &LaneRt,
        own: Vec<PortUse>,
    ) -> Option<RpcError> {
        if config.infra.external.is_empty() {
            return None;
        }
        let mut taken = self.daemon_ports();
        taken.extend(own);
        for (project, other) in self.all_lanes() {
            let st = other.st();
            let reference = st.reference(&project.name());
            // This lane's view is the one being replaced; `own` stands for it.
            let same = std::ptr::eq(other.as_ref(), lane);
            if let Some(view) = st.view.as_ref().filter(|_| !same) {
                for service in &view.services {
                    let path = format!("services.{}", service.name);
                    for port in std::iter::once(service.port).chain(service.ports.values().copied())
                    {
                        taken.push(PortUse::unit(port, &path, &reference));
                    }
                }
            }
            if let Some(infra) = st.infra_ns.as_ref().filter(|_| !same) {
                for (path, port) in infra_ports(infra) {
                    taken.push(PortUse::unit(port, &path, &reference));
                }
            }
            // Ports of the block's names the view does not show, such as
            // per-lane containers before their infra runs.
            if let Some(allocation) = &st.allocation {
                for (name, offset) in &allocation.offsets {
                    if let Some(port) = allocation.block.start.checked_add(*offset) {
                        taken.push(PortUse {
                            port,
                            holder: format!("allocated to {} in {reference}", name.unit),
                            daemon: false,
                        });
                    }
                }
            }
        }
        let pool = self.pool().ok().map(|pool| (pool.start, pool.end));
        collision(config, &taken, pool)
    }

    /// The ports devlanes listens on itself: the proxy's HTTPS and HTTP
    /// ports, bound and configured, and the DNS server's when it runs.
    fn daemon_ports(&self) -> Vec<PortUse> {
        let status = self.integ.routes.status();
        let bound = |name: &str| {
            status
                .get(name)
                .and_then(|listener| listener.get("port"))
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
        };
        let proxy = &self.opts.user.proxy;
        let https = "the devlanes proxy's HTTPS port";
        let http = "the devlanes proxy's HTTP port";
        [
            (bound("https"), https),
            (self.integ.routes.https_port(), https),
            (Some(proxy.https_port), https),
            (bound("http"), http),
            (Some(proxy.http_port), http),
            (
                self.resolver_tld().map(|_| self.opts.dns_listen.port()),
                "the devlanes DNS server's port",
            ),
        ]
        .into_iter()
        .filter_map(|(port, what)| {
            Some(PortUse {
                port: port?,
                holder: what.to_owned(),
                daemon: true,
            })
        })
        .collect()
    }

    /// `external_unit` when one of `selectors` names an `infra.external`
    /// entry of `lane`, for commands that act on a process (`start`, `stop`,
    /// `restart`, `logs`, `attach`): there is none behind it. Globs and tags
    /// never match one, since only processes are their candidates.
    pub(crate) fn refuse_external(
        project: &ProjectRt,
        lane: &LaneRt,
        selectors: &[String],
    ) -> Result<(), RpcError> {
        let st = lane.st();
        let Some(config) = st.config.loaded() else {
            return Ok(());
        };
        match selectors
            .iter()
            .find(|name| config.infra.external.contains_key(name.as_str()))
        {
            Some(name) => Err(RpcError::new(
                "external_unit",
                json!({"unit": name, "lane": st.reference(&project.name())}),
                2,
            )),
            None => Ok(()),
        }
    }

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

    /// A port in use equal to an external one is a config error naming the
    /// entry and the holder, with advice that fits it: a pool port goes to
    /// `ports.range`, another port to the unit, and devlanes' own ports get
    /// none. Ports apart are no error.
    #[test]
    fn used_ports_must_stay_apart() {
        let config = config();
        let message = |taken: &[PortUse]| {
            collision(&config, taken, Some((20000, 32767))).map(|error| {
                assert_eq!(error.code, "config_error");
                assert_eq!(error.data["path"], "infra.external.pgadmin.address");
                error.data["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
        };
        assert_eq!(
            message(&[PortUse::unit(20000, "services.api", "acme@1")]),
            None
        );
        assert_eq!(
            message(&[PortUse::unit(5050, "infra.db", "shop@0")]).as_deref(),
            Some(
                "infra.external.pgadmin.address: port 5050 is infra.db's port in shop@0. Pick another port, or change the one that unit uses"
            )
        );
        let pooled = devlanes_config::parse_project(
            "project: acme\ninfra:\n  external:\n    pgadmin: { address: '127.0.0.1:20004' }\n",
            None,
        )
        .expect("parses");
        let error = collision(
            &pooled,
            &[PortUse::unit(20004, "services.api", "acme@1")],
            Some((20000, 32767)),
        )
        .expect("collides");
        assert_eq!(
            error.data["message"],
            "infra.external.pgadmin.address: port 20004 is services.api's port in acme@1. Pick a port outside 20000-32767, the ports.range devlanes allocates from"
        );
        let own = PortUse {
            port: 5050,
            holder: "the devlanes proxy's HTTPS port".into(),
            daemon: true,
        };
        assert_eq!(
            message(&[own]).as_deref(),
            Some("infra.external.pgadmin.address: port 5050 is the devlanes proxy's HTTPS port")
        );
        assert_eq!(
            infra_ports(&json!({"db": {"port": 5050, "ports": {"5432": 5050, "x": "y"}}})),
            [("infra.db".to_owned(), 5050), ("infra.db".to_owned(), 5050)]
        );
    }
}
