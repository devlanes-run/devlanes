//! `devlanes expose`: a loopback port of a lane, made a unit of kind
//! `exposed` (`port-<port>`) without a process of its own, optionally routed
//! under a hostname. Exposed ports appear in status, are routed by this
//! machine's proxy and are mirrored to connected clients that list
//! `exposed_units`, like any unit. They live in memory only: `expose
//! --remove`, a whole-lane stop or clean, or the daemon's exit ends them.

use std::sync::Arc;

use devlanes_proto::{Context, ExposeParams, ExposedPort, RpcError};
use serde_json::{Value, json};

use crate::{
    engine::{Inner, LanePair, params},
    model::{Exposed, LaneSt, container_summaries},
};

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
        let (reference, domain) = {
            let st = lane.st();
            let reference = st.reference(&project.name());
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
            Some(host) => Some(exposed_host(
                host,
                &domain,
                &crate::remote::route_suffixes(self),
            )?),
            None => None,
        };
        let unit = Exposed::unit(port);
        self.check_port_free(&lane, port, &unit)?;
        if let Some(host) = &host {
            self.check_host_free(&reference, &unit, host)?;
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
        tracing::info!(lane = %reference, port, host = ?host, "port exposed");
        self.emit_lane(&project, &lane);
        Ok(lane_reply(&reference, &lane.st()))
    }

    /// Refuses a port another unit already advertises, on any lane of this
    /// machine, or another lane exposes: its own unit mirrors it already, or
    /// it is hidden, which exposing must not undo. Also refuses a configured
    /// unit named like the exposure.
    fn check_port_free(
        &self,
        lane: &Arc<crate::model::LaneRt>,
        port: u16,
        unit: &str,
    ) -> Result<(), RpcError> {
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
                        "port {port} belongs to {reference}:{holder}, which is mirrored and routed with its own settings"
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

    /// Refuses a hostname some other route of this machine claims, so the
    /// exposure never takes a name from a service, or the other way around.
    fn check_host_free(&self, reference: &str, unit: &str, host: &str) -> Result<(), RpcError> {
        let taken = self
            .local_route_entries()
            .into_iter()
            .find(|route| route.host == host && !(route.lane == reference && route.unit == unit));
        match taken {
            Some(route) => Err(RpcError::new(
                "host_taken",
                json!({
                    "host": host,
                    "lane": route.lane,
                    "unit": route.unit,
                    "message": format!("{host} is routed to {}:{} already", route.lane, route.unit),
                }),
                1,
            )),
            None => Ok(()),
        }
    }
}

/// The hostname `input` stands for, under `domain`: one label becomes
/// `<label>.<domain>`; a full name must be the lane's domain or below it, or
/// below one of `suffixes` (`localhost` and the configured TLD), the names
/// this machine's proxy and connected clients route. Wildcards are refused.
fn exposed_host(input: &str, domain: &str, suffixes: &[String]) -> Result<String, RpcError> {
    let invalid = |message: String| {
        RpcError::new(
            "invalid_host",
            json!({"host": input, "message": message}),
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
    let under = |suffix: &str| host == suffix || host.ends_with(&format!(".{suffix}"));
    if under(domain) || suffixes.iter().any(|suffix| under(suffix)) {
        return Ok(host);
    }
    let label = host.split('.').next().unwrap_or(&host);
    Err(RpcError::new(
        "host_not_allowed",
        json!({
            "host": host,
            "domain": domain,
            "allowed": suffixes,
            "suggestion": format!("{label}.{domain}"),
            "message": format!(
                "{host} is outside the lane's domain {domain} and the names devlanes routes ({}); use {label}.{domain}",
                suffixes.join(", ")
            ),
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

    fn suffixes() -> Vec<String> {
        vec!["localhost".into(), "test".into()]
    }

    /// One label goes under the lane's domain; full names must lie under it
    /// or under a routed suffix; wildcards and broken names are refused.
    #[test]
    fn exposed_hostnames_stay_under_routed_names() {
        let host = |input: &str| exposed_host(input, "acme-1.localhost", &suffixes());
        assert_eq!(
            host("files").ok().as_deref(),
            Some("files.acme-1.localhost")
        );
        assert_eq!(
            host("Files.Acme-1.localhost.").ok().as_deref(),
            Some("files.acme-1.localhost")
        );
        assert_eq!(host("docs.test").ok().as_deref(), Some("docs.test"));
        assert_eq!(
            host("acme-1.localhost").ok().as_deref(),
            Some("acme-1.localhost")
        );
        let outside = host("files.example.com").expect_err("outside");
        assert_eq!(outside.code, "host_not_allowed");
        assert_eq!(outside.data["suggestion"], "files.acme-1.localhost");
        assert_eq!(
            host("*.acme-1.localhost").expect_err("wildcard").code,
            "invalid_host"
        );
        assert_eq!(
            host("bad_name").expect_err("underscore").code,
            "invalid_host"
        );
        // A lane domain outside the suffixes (`lanes.vars.domain`) still takes names below it.
        assert_eq!(
            exposed_host("api", "acme.internal", &suffixes())
                .ok()
                .as_deref(),
            Some("api.acme.internal")
        );
    }
}
