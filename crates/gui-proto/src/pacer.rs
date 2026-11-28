//! Pacing of one window's stream to the viewers that watch it.
//!
//! A stream is one window in one codec. Viewers that selected the same codec
//! for a window share its encoder and its pacer; a viewer with another codec
//! gets an encoder and a pacer of its own, with its own sequence numbers and
//! keyframes.
//!
//! The pacer decides for all viewers of its stream. It keeps at most
//! [`MAX_IN_FLIGHT`] unacknowledged frames per viewer, holds only the newest
//! captured frame while it waits (older ones are dropped before they are
//! encoded, never after), and picks a target rate from how fast each viewer
//! acknowledges. The slowest viewer sets the pace; one that stops
//! acknowledging for [`STALL_AFTER`] is set aside so the others go on, and
//! gets a keyframe once it has caught up.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::{RESIZE_TIMEOUT, SessionId};

/// Frames a viewer may have unacknowledged before the window waits for it.
pub const MAX_IN_FLIGHT: usize = 3;

/// How long a viewer may sit at [`MAX_IN_FLIGHT`] before it is set aside.
/// A few seconds of silence is normal on a link that switches paths, so this
/// only stops one viewer from holding back the others, it does not end anything.
pub const STALL_AFTER: Duration = Duration::from_secs(2);

/// What the encoder aims for. Derived from acknowledgement timing; there are
/// no user settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    /// Bits per second.
    pub bitrate_bps: u32,
    /// The shortest gap between two encoded frames.
    pub frame_interval: Duration,
    /// Picture quality from 0 (lowest) to 100, for encoders that have such a knob.
    pub quality: u8,
}

/// A frame the host should encode now and send to `recipients`.
#[derive(Debug, Clone, PartialEq)]
pub struct Encode<F> {
    /// The frame, the newest captured or, for a keyframe with nothing new, the
    /// last one encoded.
    pub frame: F,
    /// The frame's sequence number, increasing per stream.
    pub seq: u64,
    /// Whether the encoder must make a keyframe.
    pub keyframe: bool,
    /// The viewers to send it to.
    pub recipients: Vec<SessionId>,
    /// The rate to encode at.
    pub target: Target,
}

/// One viewer of the window.
#[derive(Debug)]
struct Viewer {
    id: SessionId,
    /// Sequence numbers sent and not acknowledged, oldest first, with when.
    in_flight: VecDeque<(u64, Instant)>,
    needs_keyframe: bool,
    /// Set aside for not acknowledging; gets no frames until it has caught up.
    stalled: bool,
    /// The viewer's last honored keyframe request.
    key_request: KeyRequest,
    rate: RateController,
}

/// Where a viewer's last honored keyframe request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRequest {
    /// None outstanding: the next request is honored.
    Idle,
    /// Honored; its keyframe is not sent yet.
    Waiting,
    /// Its keyframe went out with this sequence number and is not acknowledged yet.
    Sent(u64),
}

/// Paces one stream: the frames of one window in one codec. `F` is the
/// captured frame, cheap to clone (an `Arc` or `Bytes`), since the last one is
/// kept for keyframes.
#[derive(Debug)]
pub struct Pacer<F> {
    viewers: Vec<Viewer>,
    pending: Option<F>,
    last: Option<F>,
    /// When the host applied a size change that `last` predates; until
    /// [`RESIZE_TIMEOUT`] after it, `last` is not used for keyframes.
    last_stale_since: Option<Instant>,
    next_seq: u64,
    last_encode: Option<Instant>,
    dropped: u64,
}

impl<F: Clone> Default for Pacer<F> {
    /// A pacer with no viewers.
    fn default() -> Self {
        Self::new()
    }
}

impl<F: Clone> Pacer<F> {
    /// A pacer with no viewers and no frame.
    pub fn new() -> Self {
        Self {
            viewers: Vec::new(),
            pending: None,
            last: None,
            last_stale_since: None,
            next_seq: 1,
            last_encode: None,
            dropped: 0,
        }
    }

    /// Adds a viewer, which gets a keyframe next.
    pub fn add_viewer(&mut self, id: SessionId) {
        self.remove_viewer(id);
        self.viewers.push(Viewer {
            id,
            in_flight: VecDeque::new(),
            needs_keyframe: true,
            stalled: false,
            key_request: KeyRequest::Idle,
            rate: RateController::new(),
        });
    }

    /// Removes a viewer, such as when its connection closes.
    pub fn remove_viewer(&mut self, id: SessionId) {
        self.viewers.retain(|viewer| viewer.id != id);
    }

    /// Whether any viewer watches the window; capture can stop when none does.
    pub fn has_viewers(&self) -> bool {
        !self.viewers.is_empty()
    }

    /// The viewer asked for a keyframe, such as after a decode error. It may
    /// never acknowledge what it already got, so the pacer stops waiting for
    /// those frames and takes the viewer back if it was set aside.
    ///
    /// Honored only when the keyframe of the viewer's previous honored request
    /// has been sent and acknowledged; until then a further request changes
    /// nothing. A viewer acknowledges a keyframe it failed to decode too, so a
    /// broken decoder still gets a new one, while a client that asks after
    /// every frame stays held to [`MAX_IN_FLIGHT`].
    pub fn request_keyframe(&mut self, id: SessionId) {
        let Some(viewer) = self.viewer(id) else {
            return;
        };
        let outstanding = match viewer.key_request {
            KeyRequest::Idle => false,
            KeyRequest::Waiting => true,
            KeyRequest::Sent(seq) => viewer.in_flight.iter().any(|(sent, _)| *sent == seq),
        };
        if outstanding {
            return;
        }
        viewer.needs_keyframe = true;
        viewer.in_flight.clear();
        viewer.stalled = false;
        viewer.key_request = KeyRequest::Waiting;
    }

    /// The host applied a size change to the window at `now`. Returns the
    /// first sequence number of frames captured after the change, for
    /// [`crate::HostMessage::ResizeApplied`].
    ///
    /// The waiting capture shows the old size and is dropped. The cached frame
    /// is kept but not used for keyframes until [`RESIZE_TIMEOUT`] has passed
    /// with nothing newer offered: a host that captures only on change gets no
    /// new frame from an app that keeps its size, and a joining viewer then
    /// still gets a picture.
    ///
    /// After this call the host must not offer frames captured before the
    /// change: they would carry sequence numbers from the returned one on.
    pub fn resize_applied(&mut self, now: Instant) -> u64 {
        self.pending = None;
        self.last_stale_since = Some(now);
        self.next_seq
    }

    /// A newly captured frame. It replaces one still waiting, which is then
    /// dropped without being encoded.
    pub fn offer(&mut self, frame: F) {
        self.last_stale_since = None;
        if self.pending.replace(frame).is_some() {
            self.dropped += 1;
        }
    }

    /// The cached frame, if it may be used for a keyframe at `now`.
    fn cached(&self, now: Instant) -> Option<&F> {
        let usable = self
            .last_stale_since
            .is_none_or(|since| now >= since + RESIZE_TIMEOUT);
        self.last.as_ref().filter(|_| usable)
    }

    /// Captured frames replaced before they were encoded.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The frame to encode now, if any.
    ///
    /// `None` while a viewer that is not set aside has [`MAX_IN_FLIGHT`] frames
    /// unacknowledged, before the frame interval of the current target has
    /// passed, or when there is nothing new and no viewer needs a keyframe.
    /// The returned frame counts as sent to every recipient at `now`.
    pub fn poll(&mut self, now: Instant) -> Option<Encode<F>> {
        self.set_aside_stalled(now);
        let mut active = self.viewers.iter().filter(|v| !v.stalled).peekable();
        active.peek()?;
        if active.any(|v| v.in_flight.len() >= MAX_IN_FLIGHT) {
            return None;
        }
        let target = self.target();
        if self
            .last_encode
            .is_some_and(|last| now < last + target.frame_interval)
        {
            return None;
        }
        let keyframe = self.viewers.iter().any(|v| !v.stalled && v.needs_keyframe);
        let frame = match self.pending.take() {
            Some(frame) => frame,
            None if keyframe => self.cached(now)?.clone(),
            None => return None,
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        let mut recipients = Vec::new();
        for viewer in self.viewers.iter_mut().filter(|v| !v.stalled) {
            viewer.in_flight.push_back((seq, now));
            if keyframe {
                viewer.needs_keyframe = false;
                if viewer.key_request == KeyRequest::Waiting {
                    viewer.key_request = KeyRequest::Sent(seq);
                }
            }
            recipients.push(viewer.id);
        }
        self.last = Some(frame.clone());
        self.last_stale_since = None;
        self.last_encode = Some(now);
        Some(Encode {
            frame,
            seq,
            keyframe,
            recipients,
            target,
        })
    }

    /// The encoder produced nothing for `seq` (an unchanged picture), so no
    /// viewer will acknowledge it.
    pub fn skipped(&mut self, seq: u64) {
        for viewer in &mut self.viewers {
            viewer.in_flight.retain(|(sent, _)| *sent != seq);
        }
    }

    /// The viewer decoded every frame up to `seq`.
    pub fn ack(&mut self, id: SessionId, seq: u64, now: Instant) {
        let Some(viewer) = self.viewer(id) else {
            return;
        };
        let mut sent_at = None;
        while let Some(&(sent, at)) = viewer.in_flight.front() {
            if sent > seq {
                break;
            }
            if sent == seq {
                sent_at = Some(at);
            }
            viewer.in_flight.pop_front();
        }
        if viewer.stalled {
            // Back only once it has caught up, so its keyframe does not queue
            // behind frames it is still working through.
            if viewer.in_flight.is_empty() {
                viewer.stalled = false;
                viewer.needs_keyframe = true;
            }
        } else if let Some(at) = sent_at {
            viewer.rate.sample(now.saturating_duration_since(at), now);
        }
    }

    /// When [`Pacer::poll`] could next return something or set a viewer aside,
    /// for the host's timer. `None` when only an acknowledgement or a new frame
    /// can change anything; `now` when a frame is due already.
    pub fn wake_at(&self, now: Instant) -> Option<Instant> {
        let active = || self.viewers.iter().filter(|v| !v.stalled);
        let stall = active()
            .filter(|v| v.in_flight.len() >= MAX_IN_FLIGHT)
            .filter_map(|v| v.in_flight.front().map(|(_, at)| *at + STALL_AFTER))
            .min();
        if stall.is_some() {
            return stall;
        }
        active().next()?;
        let interval = match self.last_encode {
            Some(last) => (last + self.target().frame_interval).max(now),
            None => now,
        };
        if self.pending.is_some() {
            return Some(interval);
        }
        if self.last.is_none() || !active().any(|v| v.needs_keyframe) {
            return None;
        }
        // A keyframe from the cache, once a resize no longer holds it back.
        let usable = self
            .last_stale_since
            .map_or(now, |since| since + RESIZE_TIMEOUT);
        Some(interval.max(usable))
    }

    /// The current target: that of the slowest viewer not set aside.
    pub fn target(&self) -> Target {
        self.viewers
            .iter()
            .filter(|v| !v.stalled)
            .map(|v| v.rate.level)
            .reduce(f64::min)
            .map_or_else(|| RateController::new().target(), level_target)
    }

    /// The viewer with `id`.
    fn viewer(&mut self, id: SessionId) -> Option<&mut Viewer> {
        self.viewers.iter_mut().find(|viewer| viewer.id == id)
    }

    /// Sets aside viewers that sat at the limit for [`STALL_AFTER`].
    fn set_aside_stalled(&mut self, now: Instant) {
        for viewer in self.viewers.iter_mut().filter(|v| !v.stalled) {
            let stuck = viewer.in_flight.len() >= MAX_IN_FLIGHT
                && viewer
                    .in_flight
                    .front()
                    .is_some_and(|(_, at)| now >= *at + STALL_AFTER);
            if stuck {
                viewer.stalled = true;
                viewer.rate.stalled(now);
            }
        }
    }
}

/// Lowest bitrate the controller goes to.
const MIN_BITRATE: f64 = 250_000.0;
/// Highest bitrate, about what 1440p UI needs at full frame rate.
const MAX_BITRATE: f64 = 20_000_000.0;
/// Frame rate at the lowest level.
const MIN_FPS: f64 = 10.0;
/// Frame rate at the highest level.
const MAX_FPS: f64 = 60.0;
/// Quality at the lowest level.
const MIN_QUALITY: f64 = 20.0;
/// Quality at the highest level.
const MAX_QUALITY: f64 = 90.0;
/// Where a new viewer starts.
const START_LEVEL: f64 = 0.5;
/// Queueing delay below which the rate rises.
const QUEUE_LOW: f64 = 0.030;
/// Queueing delay above which the rate falls hard.
const QUEUE_HIGH: f64 = 0.100;
/// Level gained per second of short queues: from the bottom to the top in ten seconds.
const RAISE_PER_SECOND: f64 = 0.1;
/// Level lost when the queue is long.
const CUT: f64 = 0.15;
/// Level lost when the queue is growing.
const GENTLE_CUT: f64 = 0.05;
/// Shortest gap between two cuts, so one burst of slow acknowledgements counts once.
const CUT_GAP: Duration = Duration::from_millis(500);
/// How long the lowest delay seen counts as the link's base delay.
const BASE_WINDOW: Duration = Duration::from_secs(10);
/// Weight of a new sample in the short average.
const SHORT_WEIGHT: f64 = 0.3;
/// Weight of a new sample in the long average; the short one above the long
/// one means delay is rising.
const LONG_WEIGHT: f64 = 0.05;
/// How far the short average must sit above the long one to count as rising.
const RISING_MARGIN: f64 = 0.005;

/// Bitrate, frame rate and quality from one number between 0 and 1. Bitrate
/// grows exponentially with it, so each step is the same relative change.
fn level_target(level: f64) -> Target {
    let level = level.clamp(0.0, 1.0);
    let bitrate = MIN_BITRATE * (MAX_BITRATE / MIN_BITRATE).powf(level);
    let fps = MIN_FPS + (MAX_FPS - MIN_FPS) * level;
    let quality = MIN_QUALITY + (MAX_QUALITY - MIN_QUALITY) * level;
    Target {
        bitrate_bps: bitrate.round() as u32,
        frame_interval: Duration::from_secs_f64(1.0 / fps),
        quality: quality.round() as u8,
    }
}

/// The rate controller of one viewer.
///
/// Each acknowledgement gives the time from sending a frame to its decode. The
/// lowest such delay over [`BASE_WINDOW`] is the link's base; the short
/// average above it is the queue the stream builds. A short queue raises the
/// level over time, a long one cuts it, and a medium one cuts it a little when
/// the trend (short average above the long one) says it is growing.
#[derive(Debug)]
struct RateController {
    level: f64,
    short: Option<f64>,
    long: Option<f64>,
    /// Increasing delays with when they were seen; the front is the minimum.
    base: VecDeque<(Instant, f64)>,
    last_raise: Option<Instant>,
    last_cut: Option<Instant>,
}

impl RateController {
    /// A controller at [`START_LEVEL`].
    fn new() -> Self {
        Self {
            level: START_LEVEL,
            short: None,
            long: None,
            base: VecDeque::new(),
            last_raise: None,
            last_cut: None,
        }
    }

    /// The target at the current level.
    fn target(&self) -> Target {
        level_target(self.level)
    }

    /// One frame's delay from sending to decode.
    fn sample(&mut self, delay: Duration, now: Instant) {
        let delay = delay.as_secs_f64();
        while self.base.back().is_some_and(|(_, d)| *d >= delay) {
            self.base.pop_back();
        }
        self.base.push_back((now, delay));
        while self.base.len() > 1
            && self
                .base
                .front()
                .is_some_and(|(at, _)| *at + BASE_WINDOW < now)
        {
            self.base.pop_front();
        }
        let base = self.base.front().map_or(delay, |(_, d)| *d);
        let short = average(self.short, delay, SHORT_WEIGHT);
        let long = average(self.long, delay, LONG_WEIGHT);
        self.short = Some(short);
        self.long = Some(long);

        let queue = short - base;
        let rising = short > long + RISING_MARGIN;
        if queue > QUEUE_HIGH {
            self.cut(CUT, now);
        } else if queue > QUEUE_LOW && rising {
            self.cut(GENTLE_CUT, now);
        } else if queue < QUEUE_LOW {
            self.raise(now);
        } else {
            // Holding: the time spent here must not count toward the next raise.
            self.last_raise = Some(now);
        }
    }

    /// The viewer stopped acknowledging: halve the level.
    fn stalled(&mut self, now: Instant) {
        self.level /= 2.0;
        self.last_cut = Some(now);
        self.last_raise = Some(now);
    }

    /// Raises the level by the time since the last raise, at most a second's worth.
    fn raise(&mut self, now: Instant) {
        let elapsed = self.last_raise.map_or(Duration::ZERO, |at| {
            now.saturating_duration_since(at)
                .min(Duration::from_secs(1))
        });
        self.level = (self.level + RAISE_PER_SECOND * elapsed.as_secs_f64()).min(1.0);
        self.last_raise = Some(now);
    }

    /// Lowers the level by `amount`, at most once per [`CUT_GAP`].
    fn cut(&mut self, amount: f64, now: Instant) {
        if self
            .last_cut
            .is_some_and(|at| now.saturating_duration_since(at) < CUT_GAP)
        {
            return;
        }
        self.level = (self.level - amount).max(0.0);
        self.last_cut = Some(now);
        self.last_raise = Some(now);
    }
}

/// An exponentially weighted moving average.
fn average(previous: Option<f64>, sample: f64, weight: f64) -> f64 {
    previous.map_or(sample, |avg| avg + weight * (sample - avg))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SessionId = SessionId(1);
    const B: SessionId = SessionId(2);

    /// `n` milliseconds.
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Offers a new frame and polls; acknowledges at once with `delay` when
    /// something was encoded. Returns the encoded frame.
    fn step(
        pacer: &mut Pacer<u32>,
        frame: u32,
        now: Instant,
        acks: &[(SessionId, Duration)],
    ) -> Option<Encode<u32>> {
        pacer.offer(frame);
        let encoded = pacer.poll(now)?;
        for (viewer, delay) in acks {
            pacer.ack(*viewer, encoded.seq, now + *delay);
        }
        Some(encoded)
    }

    #[test]
    fn a_joining_viewer_gets_a_keyframe_from_the_cache_when_nothing_is_new() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 7, t0, &[(A, ms(5))]).expect("first frame");
        assert!(first.keyframe);
        assert_eq!(first.recipients, vec![A]);

        let t1 = t0 + ms(200);
        let delta = step(&mut pacer, 8, t1, &[(A, ms(5))]).expect("second frame");
        assert!(!delta.keyframe);

        // Nothing captured since: the static window still reaches the new viewer.
        pacer.add_viewer(B);
        let t2 = t1 + ms(200);
        assert!(pacer.wake_at(t2).is_some_and(|at| at <= t2));
        let join = pacer.poll(t2).expect("keyframe for the new viewer");
        assert!(join.keyframe);
        assert_eq!(join.frame, 8);
        assert_eq!(join.recipients, vec![A, B]);
        assert_eq!(
            pacer.poll(t2 + ms(200)),
            None,
            "nothing new, nobody needs a keyframe"
        );
    }

    #[test]
    fn at_the_limit_nothing_is_encoded_and_the_newest_frame_waits() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        let mut now = t0;
        for frame in 0..MAX_IN_FLIGHT as u32 {
            assert!(step(&mut pacer, frame, now, &[]).is_some());
            now += ms(200);
        }
        for frame in 10..15 {
            assert_eq!(step(&mut pacer, frame, now, &[]), None);
        }
        assert_eq!(
            pacer.dropped(),
            4,
            "each newer frame replaced the waiting one"
        );

        pacer.ack(A, 1, now);
        let next = pacer.poll(now).expect("a slot is free");
        assert_eq!(next.frame, 14, "the newest capture, never an older one");
        assert_eq!(next.seq, 4);
    }

    #[test]
    fn a_frame_the_encoder_skipped_frees_its_slot() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        let mut now = t0;
        for frame in 0..MAX_IN_FLIGHT as u32 {
            let encoded = step(&mut pacer, frame, now, &[]).expect("frame");
            if frame == 1 {
                pacer.skipped(encoded.seq);
            }
            now += ms(200);
        }
        assert!(step(&mut pacer, 9, now, &[]).is_some());
    }

    #[test]
    fn a_steady_fast_viewer_climbs_to_the_highest_rate() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        let start = pacer.target();
        let mut now = t0;
        for frame in 0..2000 {
            step(&mut pacer, frame, now, &[(A, ms(8))]);
            now += ms(10);
        }
        let end = pacer.target();
        assert!(end.bitrate_bps > start.bitrate_bps);
        assert_eq!(end, level_target(1.0));
    }

    #[test]
    fn a_growing_queue_lowers_the_rate_and_a_short_one_brings_it_back() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        let mut now = t0;
        let mut delay = 10;
        for frame in 0..100 {
            step(&mut pacer, frame, now, &[(A, ms(delay))]);
            delay = (delay + 10).min(400);
            now += ms(50);
        }
        let congested = pacer.target();
        assert!(congested.bitrate_bps < level_target(START_LEVEL).bitrate_bps);
        assert!(congested.frame_interval > level_target(START_LEVEL).frame_interval);

        for frame in 100..400 {
            step(&mut pacer, frame, now, &[(A, ms(10))]);
            now += ms(50);
        }
        assert!(pacer.target().bitrate_bps > congested.bitrate_bps);
    }

    #[test]
    fn the_slowest_viewer_sets_the_rate() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        pacer.add_viewer(B);
        let mut now = t0;
        let mut slow = 10;
        for frame in 0..200 {
            step(&mut pacer, frame, now, &[(A, ms(5)), (B, ms(slow))]);
            slow = (slow + 10).min(500);
            now += ms(100);
        }
        let mut fast_only = Pacer::new();
        fast_only.add_viewer(A);
        let mut now = t0;
        for frame in 0..200 {
            step(&mut fast_only, frame, now, &[(A, ms(5))]);
            now += ms(100);
        }
        assert!(pacer.target().bitrate_bps < fast_only.target().bitrate_bps);
    }

    #[test]
    fn a_viewer_that_stops_acknowledging_is_set_aside_and_rejoins_with_a_keyframe() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        pacer.add_viewer(B);
        let mut now = t0;
        let mut b_last = 0;
        for frame in 0..MAX_IN_FLIGHT as u32 {
            let encoded = step(&mut pacer, frame, now, &[(A, ms(5))]).expect("frame");
            b_last = encoded.seq;
            now += ms(200);
        }
        // B holds the window back until it has been stuck for STALL_AFTER.
        assert_eq!(step(&mut pacer, 50, now, &[]), None);
        let stall_at = pacer.wake_at(now).expect("a stall deadline");
        assert_eq!(stall_at, t0 + STALL_AFTER);
        assert_eq!(pacer.poll(stall_at - ms(1)), None);
        let without_b = pacer.poll(stall_at).expect("A goes on alone");
        assert_eq!(without_b.recipients, vec![A]);
        pacer.ack(A, without_b.seq, stall_at);

        // An acknowledgement after B was set aside still brings it back.
        now = stall_at + ms(500);
        pacer.ack(B, b_last, now);
        let rejoin = step(&mut pacer, 60, now, &[]).expect("frame for both");
        assert!(rejoin.keyframe);
        assert_eq!(rejoin.recipients, vec![A, B]);
    }

    #[test]
    fn a_viewer_that_fails_to_decode_recovers_through_a_keyframe_request() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        pacer.add_viewer(B);
        let mut now = t0;
        for frame in 0..MAX_IN_FLIGHT as u32 {
            step(&mut pacer, frame, now, &[(A, ms(5))]).expect("frame");
            now += ms(200);
        }
        // B failed on its first frame and cannot acknowledge the deltas after it.
        pacer.request_keyframe(B);
        let recovery = step(&mut pacer, 70, now, &[]).expect("not held back by B");
        assert!(recovery.keyframe);
        assert_eq!(recovery.recipients, vec![A, B]);

        // A broken decoder fails on the keyframe too, discards and acknowledges
        // it, and its next request is honored.
        let now = now + ms(200);
        pacer.ack(A, recovery.seq, now);
        let delta = step(&mut pacer, 71, now, &[(A, ms(5))]).expect("delta");
        assert!(!delta.keyframe);
        pacer.ack(B, recovery.seq, now);
        pacer.request_keyframe(B);
        let again = step(&mut pacer, 72, now + ms(200), &[]).expect("frame");
        assert!(again.keyframe);
        assert_eq!(again.recipients, vec![A, B]);
    }

    #[test]
    fn a_viewer_that_asks_for_keyframes_without_acknowledging_is_still_held_back() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        pacer.add_viewer(B);
        let mut now = t0;
        let mut keyframes = 0;
        let mut sent_to_b = 0;
        for frame in 0..20 {
            pacer.request_keyframe(B);
            if let Some(encoded) = step(&mut pacer, frame, now, &[(A, ms(5))]) {
                keyframes += usize::from(encoded.keyframe);
                sent_to_b += usize::from(encoded.recipients.contains(&B));
            }
            now += ms(100);
        }
        // Only the join keyframe answers B's requests; then B sits at the limit.
        assert_eq!(keyframes, 1);
        assert_eq!(sent_to_b, MAX_IN_FLIGHT);
        // Set aside after STALL_AFTER, and asking again does not bring it back.
        let aside = pacer.poll(now).expect("A goes on alone");
        assert_eq!(aside.recipients, vec![A]);
        pacer.ack(A, aside.seq, now);
        pacer.request_keyframe(B);
        let next = step(&mut pacer, 99, now + ms(100), &[]).expect("frame for A");
        assert!(!next.keyframe);
        assert_eq!(next.recipients, vec![A]);
    }

    #[test]
    fn frames_after_a_resize_start_at_the_returned_sequence_number() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        step(&mut pacer, 1, t0, &[(A, ms(5))]).expect("frame");
        pacer.offer(2);
        let from = pacer.resize_applied(t0 + ms(100));
        assert_eq!(
            pacer.poll(t0 + ms(200)),
            None,
            "the old-size capture is gone"
        );
        let next = step(&mut pacer, 3, t0 + ms(400), &[]).expect("new-size frame");
        assert_eq!((next.frame, next.seq), (3, from));
    }

    #[test]
    fn a_viewer_joining_after_a_resize_with_no_new_capture_gets_the_old_frame_later() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new();
        pacer.add_viewer(A);
        step(&mut pacer, 1, t0, &[(A, ms(5))]).expect("frame");
        let applied = t0 + ms(100);
        pacer.resize_applied(applied);
        pacer.add_viewer(B);
        assert_eq!(
            pacer.poll(applied + ms(500)),
            None,
            "the old size may still change"
        );
        assert_eq!(
            pacer.wake_at(applied + ms(500)),
            Some(applied + RESIZE_TIMEOUT)
        );
        let join = pacer
            .poll(applied + RESIZE_TIMEOUT)
            .expect("the cached frame");
        assert!(join.keyframe);
        assert_eq!((join.frame, join.recipients), (1, vec![A, B]));
    }
}
