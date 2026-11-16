use std::{fmt::Write, time::Duration};

use hyper::header::{HeaderMap, HeaderValue};

/// Tells the proxy how far away the box behind a remote route is, keyed by
/// `Route::owner` (the lane reference on the box, such as `acme@0`).
pub trait BoxLatency: Send + Sync {
    /// The latest round trip to the box that mirrors lane `owner`, if known.
    fn round_trip(&self, owner: &str) -> Option<Duration>;
}

/// Where the proxy's time on one request went, for its `Server-Timing` field.
#[derive(Debug, Default)]
pub(crate) struct Timings {
    /// Held for a starting service, a reconnecting box or a thawing lane.
    pub wait: Duration,
    /// Dialing the service, refused retries included; `None` on a reused connection.
    pub connect: Option<Duration>,
    /// From sending the request to the service's response head; `None` when
    /// nothing was sent.
    pub upstream: Option<Duration>,
    /// The latest ping round trip to the box of a remote route, which the
    /// upstream time includes.
    pub tunnel: Option<Duration>,
}

impl Timings {
    /// Appends one `Server-Timing` field after the service's own, which stay as
    /// they are. `devlanes` is `total` minus the upstream time: what the
    /// proxy itself added.
    pub(crate) fn append(&self, total: Duration, headers: &mut HeaderMap) {
        let own = total.saturating_sub(self.upstream.unwrap_or_default());
        let mut value = format!("devlanes;dur={}", millis(own));
        if !self.wait.is_zero() {
            let _ = write!(value, ", devlanes-wait;dur={}", millis(self.wait));
        }
        if let Some(connect) = self.connect {
            let _ = write!(value, ", devlanes-connect;dur={}", millis(connect));
        }
        if let Some(tunnel) = self.tunnel {
            let _ = write!(
                value,
                ", devlanes-tunnel;dur={};desc=\"ping estimate\"",
                millis(tunnel)
            );
        }
        if let Some(upstream) = self.upstream {
            let _ = write!(value, ", devlanes-upstream;dur={}", millis(upstream));
        }
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.append("server-timing", value);
        }
    }
}

/// Milliseconds with one decimal, as `Server-Timing` durations are read.
fn millis(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}
