//! `devlanes expose`: a loopback port of a lane, made a unit of kind
//! `exposed` (`port-<port>`) without a process of its own, optionally routed
//! under a hostname of the lane's domain. Exposed ports appear in status, are
//! routed by this machine's proxy and are mirrored to connected clients that
//! list `exposed_units`, like any unit. They live in memory only: `expose
//! --remove`, a whole-lane stop (not a transient one) or clean, or the
//! daemon's exit ends them.

use std::sync::{Arc, Mutex};

use devlanes_proto::{Context, ExposeParams, ExposedPort, RpcError};
use serde_json::{Value, json};

use crate::{
    engine::{Inner, LanePair, params},
    model::{Exposed, LaneRt, LaneSt, container_summaries, lock},
};

/// Held from the checks of an exposure to its insertion, so two `expose`
/// requests, of one lane or two, never both take a port or a hostname.
static EXPOSING: Mutex<()> = Mutex::new(());

impl Inner {
    /// `expose`: adds or replaces the exposure of a port of the context's
    /// lane, removes it with `remove`, or without a port lists the exposed
    /// ports of the request's lanes (every lane with `all`).
    pub(crate) async fn expose_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: ExposeParams = params(value)?;
        let Some(port) = args.port else {
            if args.remove || args.host.is_some() {
                return Err(bad_request("name the port to expose or remove"));
            }
            let lanes = self.scope_lanes(context, args.all).await?;
            return Ok(
                json!({"items": exposed_of(&lanes), "truncated": false, "next_cursor": null}),
            );
        };
        if port == 0 {
            return Err(bad_request("port 0 cannot be exposed"));
        }
        let (project, lane) = self.target_lane(context).await?;
        if args.remove {
            let removed = lane.st().exposed.remove(&port).is_some();
            let reference = lane.st().reference(&project.name());
            if !removed {
                return Err(RpcError::new(
                    "not_exposed",
                    json!({
                        "port": port,
                        "lane": reference,
                        "message": format!("port {port} is not exposed in {reference}"),
                    }),
                    3,
                ));
            }
            tracing::info!(lane = %reference, port, "port no longer exposed");
            self.emit_lane(&project, &lane);
            return Ok(lane_reply(&reference, &lane.st()));
        }
        if lane.st().view.is_none() {
            self.refresh_views(&project).await;
        }
        self.check_daemon_port(port)?;
        let (reference, host) = {
            let _exposing = lock(&EXPOSING);
            let (reference, domain) = {
                let st = lane.st();
                let reference = st.reference(&project.name());
                // A clean, reset or whole stop under way drops what the lane
                // exposes once it is done, so an exposure added now would vanish.
                if st.closing || st.busy == Some("stopping") {
                    return Err(RpcError::new("lane_busy", json!({"lane": reference}), 1));
                }
                match (&st.record, &st.view) {
                    (Some(_), Some(view)) => (reference, view.domain.clone()),
                    _ => {
                        return Err(st.last_error.clone().unwrap_or_else(|| {
                            RpcError::new("unassigned", json!({"lane": reference}), 1)
                        }));
                    }
                }
            };
            let host = match &args.host {
                Some(host) => Some(exposed_host(host, &domain, port)?),
                None => None,
            };
            let unit = Exposed::unit(port);
            self.check_port_free(&lane, port, &unit)?;
            if let Some(host) = &host {
                self.check_host_free(&lane, (&reference, &domain), &unit, host)?;
            }
            let url = match &host {
                Some(host) => devlanes_resolve::https_url(host, self.integ.routes.https_port()),
                None => format!("http://127.0.0.1:{port}"),
            };
            let exposed = Exposed {
                host: host.clone(),
                url,
                since_ms: crate::units::now_ms(),
                actor: context.actor.clone(),
            };
            lane.st().exposed.insert(port, exposed);
            (reference, host)
        };
        tracing::info!(lane = %reference, port, host = ?host, "port exposed");
        self.emit_lane(&project, &lane);
        Ok(lane_reply(&reference, &lane.st()))
    }

    /// Refuses the ports this daemon listens on itself, its proxy's and its
    /// DNS server's: routing them to a lane would loop or shadow devlanes.
    fn check_daemon_port(&self, port: u16) -> Result<(), RpcError> {
        let status = self.integ.routes.status();
        let bound = |name: &str| {
            status
                .get(name)
                .and_then(|listener| listener.get("port"))
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
        };
        let proxy = &self.opts.user.proxy;
        let own = [
            (bound("https"), "the devlanes proxy's HTTPS port"),
            (
                self.integ.routes.https_port(),
                "the devlanes proxy's HTTPS port",
            ),
            (Some(proxy.https_port), "the devlanes proxy's HTTPS port"),
            (bound("http"), "the devlanes proxy's HTTP port"),
            (Some(proxy.http_port), "the devlanes proxy's HTTP port"),
            (
                self.resolver_tld().map(|_| self.opts.dns_listen.port()),
                "the devlanes DNS server's port",
            ),
        ];
        match own.into_iter().find(|(taken, _)| *taken == Some(port)) {
            Some((_, what)) => Err(RpcError::new(
                "port_reserved",
                json!({"port": port, "message": format!("port {port} is {what}, which devlanes listens on itself")}),
                1,
            )),
            None => Ok(()),
        }
    }

    /// Refuses a port another unit already advertises, on any lane of this
    /// machine, or another lane exposes: its own unit mirrors it already, or
    /// it is hidden, which exposing must not undo. Also refuses a configured
    /// unit named like the exposure.
    fn check_port_free(&self, lane: &Arc<LaneRt>, port: u16, unit: &str) -> Result<(), RpcError> {
        for (project, other) in self.all_lanes() {
            let st = other.st();
            let reference = st.reference(&project.name());
            let same = Arc::ptr_eq(&other, lane);
            let mut holders: Vec<String> = Vec::new();
            for (name, rec) in &st.units {
                if rec.ports.values().any(|p| *p == port) {
                    holders.push(name.clone());
                }
            }
            if let Some(view) = &st.view {
                for service in &view.services {
                    if service.port == port || service.ports.values().any(|p| *p == port) {
                        holders.push(service.unit.clone());
                    }
                }
            }
            for container in container_summaries(&st) {
                if container.ports.values().any(|p| *p == port) {
                    holders.push(container.name.clone());
                }
            }
            if !same && st.exposed.contains_key(&port) {
                holders.push(unit.to_owned());
            }
            if let Some(holder) = holders.first() {
                return Err(port_taken(
                    port,
                    &reference,
                    holder,
                    format!(
                        "port {port} is already {reference}:{holder}'s; that unit is routed (and mirrored to clients) with its own settings"
                    ),
                ));
            }
            let named = st.units.contains_key(unit)
                || st
                    .view
                    .as_ref()
                    .is_some_and(|view| view.services.iter().any(|s| s.unit == unit));
            if same && named {
                return Err(port_taken(
                    port,
                    &reference,
                    unit,
                    format!("{reference} already has a unit named {unit}"),
                ));
            }
        }
        Ok(())
    }

    /// Refuses a hostname another route of this machine claims, one that
    /// another lane's wildcard route matches, and one under another lane's
    /// domain (a lane domain below this one's): the exposure never takes a
    /// name from a service or another lane, nor the other way around.
    fn check_host_free(
        &self,
        lane: &Arc<LaneRt>,
        (reference, domain): (&str, &str),
        unit: &str,
        host: &str,
    ) -> Result<(), RpcError> {
        let taken = |route: &crate::integrations::RouteEntry, message: String| {
            RpcError::new(
                "host_taken",
                json!({"host": host, "lane": route.lane, "unit": route.unit, "message": message}),
                1,
            )
        };
        for route in self.local_route_entries() {
            if route.lane == reference && route.unit == unit {
                continue;
            }
            if route.host == host {
                return Err(taken(
                    &route,
                    format!("{host} is routed to {}:{} already", route.lane, route.unit),
                ));
            }
            let matched = route
                .host
                .strip_prefix("*.")
                .is_some_and(|rest| host.ends_with(&format!(".{rest}")));
            if matched && route.lane != reference {
                return Err(taken(
                    &route,
                    format!(
                        "{host} matches {}, which {}:{} routes",
                        route.host, route.lane, route.unit
                    ),
                ));
            }
        }
        for (project, other) in self.all_lanes() {
            if Arc::ptr_eq(&other, lane) {
                continue;
            }
            let st = other.st();
            let Some(view) = &st.view else { continue };
            let other_domain = &view.domain;
            // Only a domain below this lane's can hold names this lane may use.
            let below = other_domain.ends_with(&format!(".{domain}"));
            if below && (host == other_domain || host.ends_with(&format!(".{other_domain}"))) {
                let other_ref = st.reference(&project.name());
                return Err(RpcError::new(
                    "host_taken",
                    json!({
                        "host": host,
                        "lane": other_ref,
                        "message": format!("{host} is under the domain of {other_ref}, {other_domain}"),
                    }),
                    1,
                ));
            }
        }
        Ok(())
    }
}

/// The hostname `input` stands for, below the lane's `domain`: one label
/// becomes `<label>.<domain>`, and a full name must lie below the domain.
/// Wildcards are refused. `port` goes into the refusal for its hint.
fn exposed_host(input: &str, domain: &str, port: u16) -> Result<String, RpcError> {
    let invalid = |message: String| {
        RpcError::new(
            "invalid_host",
            json!({"host": input, "port": port, "message": message}),
            2,
        )
    };
    let host = devlanes_config::normalize_host(input).map_err(invalid)?;
    if host.starts_with("*.") {
        return Err(invalid(format!(
            "`{input}`: an exposed port takes one hostname, not a wildcard"
        )));
    }
    let host = if host.contains('.') {
        host
    } else {
        devlanes_config::normalize_host(&format!("{host}.{domain}")).map_err(invalid)?
    };
    if host.ends_with(&format!(".{domain}")) {
        return Ok(host);
    }
    let label = host.split('.').next().unwrap_or(&host);
    let suggestion = format!("{label}.{domain}");
    Err(RpcError::new(
        "host_not_allowed",
        json!({
            "host": host,
            "port": port,
            "domain": domain,
            "suggestion": suggestion,
            "message": format!("{host} is not under the lane's domain {domain}"),
        }),
        2,
    ))
}

/// `port_taken`: `port` or the exposure's name is used by `unit` of `lane`.
fn port_taken(port: u16, lane: &str, unit: &str, message: String) -> RpcError {
    RpcError::new(
        "port_taken",
        json!({"port": port, "lane": lane, "unit": unit, "message": message}),
        1,
    )
}

/// `bad_request` with `message`.
fn bad_request(message: &str) -> RpcError {
    RpcError::new("bad_request", json!({"message": message}), 2)
}

/// The reply to an add or remove: the lane and what it exposes now.
fn lane_reply(reference: &str, st: &LaneSt) -> Value {
    let exposed: Vec<ExposedPort> = st
        .exposed
        .iter()
        .map(|(port, exposed)| exposed.listed(reference, *port))
        .collect();
    json!({"lane": reference, "exposed": exposed})
}

/// Every exposed port of `lanes`, in lane order.
fn exposed_of(lanes: &[LanePair]) -> Vec<ExposedPort> {
    let mut items: Vec<ExposedPort> = lanes
        .iter()
        .flat_map(|(project, lane)| {
            let st = lane.st();
            let reference = st.reference(&project.name());
            st.exposed
                .iter()
                .map(|(port, exposed)| exposed.listed(&reference, *port))
                .collect::<Vec<_>>()
        })
        .collect();
    items.sort_by(|a, b| (&a.lane, a.port).cmp(&(&b.lane, b.port)));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One label goes below the lane's domain; a full name must lie below
    /// it; the domain itself, other names, wildcards and broken names are
    /// refused, the name outside with a suggestion and the port.
    #[test]
    fn exposed_hostnames_stay_below_the_lane_domain() {
        let host = |input: &str| exposed_host(input, "acme-1.localhost", 8000);
        assert_eq!(
            host("files").ok().as_deref(),
            Some("files.acme-1.localhost")
        );
        assert_eq!(
            host("Files.Acme-1.localhost.").ok().as_deref(),
            Some("files.acme-1.localhost")
        );
        assert_eq!(
            host("a.files.acme-1.localhost").ok().as_deref(),
            Some("a.files.acme-1.localhost")
        );
        for outside in ["files.example.com", "docs.localhost", "acme-1.localhost"] {
            let refused = host(outside).expect_err(outside);
            assert_eq!(refused.code, "host_not_allowed", "{outside}");
            assert_eq!(refused.data["port"], 8000);
        }
        let refused = host("docs.test").expect_err("another suffix");
        assert_eq!(refused.data["suggestion"], "docs.acme-1.localhost");
        assert_eq!(
            refused.data["message"],
            "docs.test is not under the lane's domain acme-1.localhost"
        );
        assert_eq!(
            host("*.acme-1.localhost").expect_err("wildcard").code,
            "invalid_host"
        );
        assert_eq!(
            host("bad_name").expect_err("underscore").code,
            "invalid_host"
        );
    }
}
