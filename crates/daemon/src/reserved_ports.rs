//! `doctor`'s view of the Windows reserved TCP port ranges that
//! `devlanes-ports-windows` reads: which of them overlap `ports.range`, the
//! proxy ports and mirrored ports.

use std::collections::BTreeSet;

use serde_json::{Value, json};

pub use devlanes_ports_windows::{LIST_COMMAND, RESERVED, Reading};

/// The fallback ports the proxy binds when its configured ones fail.
const PROXY_FALLBACKS: [(&str, u16); 2] = [
    ("http", devlanes_config::HTTP_FALLBACK_PORT),
    ("https", devlanes_config::HTTPS_FALLBACK_PORT),
];

/// `doctor`'s `reserved_ports`: `{ranges, pool, pool_overlaps, proxy,
/// mirrored, error?, command}`, or `{error, command}` when no family could be
/// read. `pool` is the inclusive `ports.range` (`None` when invalid, which the
/// user config check reports); `pool_overlaps` are the ranges overlapping it;
/// `proxy` the proxy ports inside a range, configured or fallback
/// (`[{listener, port, fallback}]`); `mirrored` the mirrored ports inside one.
pub fn report(
    reading: &Reading,
    pool: Option<(u16, u16)>,
    proxy: [(&'static str, u16); 2],
    mirrored: &BTreeSet<u16>,
) -> Value {
    let error = (!reading.errors.is_empty()).then(|| reading.errors.join("; "));
    if !reading.any_read() {
        return json!({"error": error, "command": LIST_COMMAND});
    }
    let range = |&(start, end): &(u16, u16)| json!({"start": start, "end": end});
    let pool_overlaps: Vec<Value> = pool
        .map(|(first, last)| {
            reading
                .ranges
                .iter()
                .filter(|(start, end)| *start <= last && first <= *end)
                .map(range)
                .collect()
        })
        .unwrap_or_default();
    let mut proxy_ports: Vec<(&str, u16, bool)> = Vec::new();
    for (listener, port) in proxy {
        proxy_ports.push((listener, port, false));
        if let Some((_, fallback)) = PROXY_FALLBACKS
            .iter()
            .find(|(name, fallback)| *name == listener && *fallback != port)
        {
            proxy_ports.push((listener, *fallback, true));
        }
    }
    let proxy: Vec<Value> = proxy_ports
        .into_iter()
        .filter(|(_, port, _)| reading.reserves(*port))
        .map(|(listener, port, fallback)| {
            json!({"listener": listener, "port": port, "fallback": fallback})
        })
        .collect();
    let mirrored: Vec<u16> = mirrored
        .iter()
        .copied()
        .filter(|port| reading.reserves(*port))
        .collect();
    json!({
        "ranges": reading.ranges.iter().map(range).collect::<Vec<_>>(),
        "pool": pool.map(|pool| range(&pool)),
        "pool_overlaps": pool_overlaps,
        "proxy": proxy,
        "mirrored": mirrored,
        "error": error,
        "command": LIST_COMMAND,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reading with `ranges` and every family read.
    fn read_ok(ranges: Vec<(u16, u16)>) -> Reading {
        Reading {
            ranges,
            errors: Vec::new(),
        }
    }

    /// Ranges touching either end of the pool overlap it; only mirrored and
    /// proxy ports inside a range are reported, fallbacks included.
    #[test]
    fn report_lists_overlaps() {
        let reading = read_ok(vec![(80, 80), (8440, 8449), (27339, 27339), (32767, 33000)]);
        let report = report(
            &reading,
            Some((20000, 32767)),
            [("http", 80), ("https", 443)],
            &BTreeSet::from([27339, 27340]),
        );
        assert_eq!(
            report["pool_overlaps"],
            json!([{"start": 27339, "end": 27339}, {"start": 32767, "end": 33000}])
        );
        assert_eq!(report["mirrored"], json!([27339]));
        assert_eq!(
            report["proxy"],
            json!([
                {"listener": "http", "port": 80, "fallback": false},
                {"listener": "https", "port": 8443, "fallback": true},
            ])
        );
        assert_eq!(report["error"], Value::Null);
        assert_eq!(report["command"], LIST_COMMAND);

        // A configured port equal to its fallback is listed once.
        let report = super::report(
            &reading,
            None,
            [("http", 8080), ("https", 8443)],
            &BTreeSet::new(),
        );
        assert_eq!(
            report["proxy"],
            json!([{"listener": "https", "port": 8443, "fallback": false}])
        );
        assert_eq!(report["pool"], Value::Null);
        assert_eq!(report["pool_overlaps"], json!([]));
    }

    /// One failed family is reported beside the other's ranges; with none read
    /// only the error is left.
    #[test]
    fn failed_reads_are_reported() {
        let partial = Reading {
            ranges: vec![(20000, 20010)],
            errors: vec!["ipv6: netsh failed".into()],
        };
        let report = report(
            &partial,
            Some((20000, 20005)),
            [("http", 80), ("https", 443)],
            &BTreeSet::new(),
        );
        assert_eq!(report["error"], "ipv6: netsh failed");
        assert_eq!(
            report["pool_overlaps"],
            json!([{"start": 20000, "end": 20010}])
        );
        let failed = Reading {
            ranges: Vec::new(),
            errors: vec!["ipv4: x".into(), "ipv6: y".into()],
        };
        assert_eq!(
            super::report(
                &failed,
                None,
                [("http", 80), ("https", 443)],
                &BTreeSet::new()
            ),
            json!({"error": "ipv4: x; ipv6: y", "command": LIST_COMMAND})
        );
    }
}
