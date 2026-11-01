//! Monotonic time behind a trait, so schedulers such as the start gate can be
//! driven by a manual clock in tests.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;

/// A monotonic clock. Readings are offsets from an arbitrary origin fixed when
/// the clock was created; only differences between readings mean anything.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Current offset from the clock's origin.
    fn now(&self) -> Duration;

    /// Completes once [`Clock::now`] reaches `deadline`; at once if it already has.
    async fn sleep_until(&self, deadline: Duration);
}

/// The tokio clock. Under `tokio::time::pause` it follows the paused test time.
#[derive(Debug, Clone, Copy)]
pub struct TokioClock {
    origin: tokio::time::Instant,
}

impl TokioClock {
    /// A clock whose origin is the current instant.
    pub fn new() -> Self {
        Self {
            origin: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioClock {
    /// Same as [`TokioClock::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for TokioClock {
    /// Time elapsed since the clock was created.
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    /// Sleeps with `tokio::time::sleep_until`.
    async fn sleep_until(&self, deadline: Duration) {
        tokio::time::sleep_until(self.origin + deadline).await;
    }
}

/// A clock that only moves when [`ManualClock::advance`] is called. Test double:
/// sleepers wake on the advance that reaches their deadline.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: std::sync::Arc<watch::Sender<Duration>>,
}

impl ManualClock {
    /// A clock standing at zero.
    pub fn new() -> Self {
        Self {
            now: std::sync::Arc::new(watch::channel(Duration::ZERO).0),
        }
    }

    /// Moves time forward by `by` and wakes every sleeper whose deadline passed.
    pub fn advance(&self, by: Duration) {
        self.now.send_modify(|now| *now += by);
    }
}

impl Default for ManualClock {
    /// Same as [`ManualClock::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for ManualClock {
    /// The time reached by all advances so far.
    fn now(&self) -> Duration {
        *self.now.borrow()
    }

    /// Waits for the advance that reaches `deadline`.
    async fn sleep_until(&self, deadline: Duration) {
        let mut now = self.now.subscribe();
        // The sender lives as long as `self`, so `wait_for` only fails if the
        // clock is gone, and then nobody can advance it anyway.
        let _ = now.wait_for(|now| *now >= deadline).await;
    }
}

/// `YYYYMMDDTHHMMSSZ` for Unix seconds, in UTC, for file names that sort by
/// time.
pub fn utc_stamp(seconds: u64) -> String {
    // Days to civil date (Howard Hinnant's algorithm), proleptic Gregorian.
    let days = i64::try_from(seconds / 86_400).unwrap_or_default() + 719_468;
    let rest = seconds % 86_400;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Known instants, including a leap day.
    fn utc_stamps() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
        assert_eq!(utc_stamp(1_792_595_045), "20261021T150405Z");
    }

    #[tokio::test]
    async fn manual_clock_wakes_sleepers_on_advance() {
        let clock = ManualClock::new();
        let sleeper = {
            let clock = clock.clone();
            tokio::spawn(async move { clock.sleep_until(Duration::from_secs(2)).await })
        };
        clock.advance(Duration::from_secs(1));
        tokio::task::yield_now().await;
        assert!(!sleeper.is_finished());
        clock.advance(Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(5), sleeper)
            .await
            .expect("sleeper wakes")
            .expect("sleeper task");
        assert_eq!(clock.now(), Duration::from_secs(2));
    }
}
