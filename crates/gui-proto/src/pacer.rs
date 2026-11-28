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

/// How long the picture must stay unchanged, with every frame acknowledged,
/// before a lossy stream sends its sharp-text refinement.
const REFINE_AFTER: Duration = Duration::from_millis(400);

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
    /// Whether this is the sharp-text refinement: the last frame again, once
    /// the picture stopped changing, coded at the finer quality in `target`.
    pub refine: bool,
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
    /// Bytes a second this viewer's link carries, from large frames (keyframes,
    /// refinements, frames whose acknowledgement took several round trips):
    /// their size over the time from sending to acknowledgement less the
    /// base delay. `None` until one was measured.
    throughput: Option<f64>,
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
    /// Whether the stream's codec is lossy, so a still picture gets a refinement.
    refinement: bool,
    /// The quality the picture in `last` was refined at; `None` while it
    /// was not.
    refined_at: Option<u8>,
    /// Recent frames: what was asked of each and, once the host reported
    /// it, its size, for throughput samples and refinement estimates.
    sent: VecDeque<Sent>,
    /// The size and quality of the stream's last keyframe at its current
    /// size, which a refinement's size is estimated from.
    last_keyframe: Option<(usize, u8)>,
    /// The first sequence number at the window's current size.
    size_from: u64,
    /// What refinements of this stream really cost against their raw
    /// estimate ([`Pacer::estimate`]), a running average.
    refine_ratio: f64,
    /// When a refinement was last sent as a probe of a link too slow for one.
    last_probe: Option<Instant>,
}

/// One frame the pacer asked for.
#[derive(Debug, Clone, Copy)]
struct Sent {
    seq: u64,
    quality: u8,
    refine: bool,
    /// For a refinement, the raw estimate of its bytes.
    estimate: Option<f64>,
    /// The bytes sent and whether it was a keyframe, once reported.
    size: Option<(usize, bool)>,
}

/// The longest a refinement may occupy the slowest viewer's link; a slower
/// link gets a coarser refinement or none, since the next change would wait
/// behind it.
const REFINE_LINK_BUDGET: Duration = Duration::from_millis(300);

/// How many quality points apart the refinement qualities tried are, from
/// 100 down.
const REFINE_QUALITY_STEP: u8 = 20;

/// What a refinement costs against a keyframe at its quantizer, before a
/// refinement of the stream measured it: a refinement is a predicted frame
/// of the same picture. Microsoft's and NVIDIA's encoders measured 0.26 on
/// text at 640x360.
const REFINE_RATIO: f64 = 0.3;

/// When no refinement fits the link, or a viewer's link is not measured,
/// the lowest one is still sent this often, so the link is measured again
/// and refinements come back when it is faster.
const PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// How many frames the pacer remembers for throughput samples.
const KEPT_FRAMES: usize = 16;

/// The quantizer steps a refinement is at most finer than its stream: a
/// refinement of text at 640x360 cost Microsoft's and NVIDIA's encoders
/// twice the stream's keyframe at 21 steps finer and under 1.5 times it at
/// 15, which still took three quarters of the blur away.
pub const MAX_REFINE_STEPS: u8 = 15;

/// The H.264 quantizer (0 best to 51 worst) for a quality from 0 to 100: 44,
/// blocky but small, at 0, down to 18 at 100. The encoders code at it; six
/// steps double a frame's size.
pub fn quantizer(quality: u8) -> u8 {
    44 - (u32::from(quality.min(100)) * 26 / 100) as u8
}

/// The quantizer of a refinement at `quality` of a stream coded at
/// `stream_qp`: at most [`MAX_REFINE_STEPS`] finer than the stream, and
/// never coarser.
pub fn refine_qp(quality: u8, stream_qp: u8) -> u8 {
    quantizer(quality)
        .max(stream_qp.saturating_sub(MAX_REFINE_STEPS))
        .min(stream_qp)
}

/// A frame whose acknowledgement took this many times the link's base delay
/// is large enough that the time measures the link's throughput.
const LARGE_FRAME_DELAY: f64 = 3.0;

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
            refinement: false,
            refined_at: None,
            sent: VecDeque::new(),
            last_keyframe: None,
            size_from: 1,
            refine_ratio: REFINE_RATIO,
            last_probe: None,
        }
    }

    /// The same pacer for a lossy codec: once the picture has not changed
    /// for 400 ms and every viewer acknowledged the last frame, it asks for
    /// that frame again as a refinement at the best quality the slowest
    /// viewer's link carries in 300 ms. When none fits, or a link is not
    /// measured yet, the lowest refinement still goes out at most every 30
    /// seconds, which measures the links again; a refinement that measured
    /// them fast is followed by a finer one.
    pub fn with_refinement(mut self) -> Self {
        self.refinement = true;
        self
    }

    /// The host sent frame `seq` as `bytes` bytes, a keyframe or not. Sizes
    /// give the throughput of each viewer when it acknowledges a large frame
    /// and estimate what a refinement would cost; a refinement's own size
    /// calibrates those estimates.
    pub fn encoded(&mut self, seq: u64, bytes: usize, keyframe: bool) {
        let Some(sent) = self.sent.iter_mut().find(|s| s.seq == seq) else {
            return;
        };
        sent.size = Some((bytes, keyframe));
        if keyframe && seq >= self.size_from {
            self.last_keyframe = Some((bytes, sent.quality));
        }
        if let Some(estimate) = sent.estimate.filter(|e| *e > 0.0) {
            let ratio = (bytes as f64 / estimate).clamp(0.02, 4.0);
            self.refine_ratio = average(Some(self.refine_ratio), ratio, 0.5);
        }
    }

    /// The raw estimate of a refinement's bytes at `quality` of a stream
    /// at `stream_quality`: what a keyframe would cost at the refinement's
    /// quantizer, scaled in quantizer steps (six double the size) from the
    /// last keyframe or, when larger, from a recent frame at this size,
    /// since a keyframe of an early, emptier picture understates the
    /// picture now.
    fn estimate(&self, stream_quality: u8, quality: u8) -> Option<f64> {
        let target = refine_qp(quality, quantizer(stream_quality));
        let scaled = |bytes: usize, at: u8| {
            let steps = f64::from(quantizer(at)) - f64::from(target);
            bytes as f64 * (steps / 6.0).exp2()
        };
        let recent = self
            .sent
            .iter()
            .filter(|s| s.seq >= self.size_from && !s.refine)
            .filter_map(|s| s.size.map(|(bytes, _)| scaled(bytes, s.quality)));
        self.last_keyframe
            .map(|(bytes, at)| scaled(bytes, at))
            .into_iter()
            .chain(recent)
            .reduce(f64::max)
    }

    /// The refinement to send now, its quality and whether it is a probe:
    /// the finest of 100, 80, 60 and so on down to one step above the
    /// stream's quality whose calibrated estimate the slowest viewer's link
    /// carries within [`REFINE_LINK_BUDGET`], when finer than the one the
    /// picture had; otherwise the lowest step as a probe, when the picture
    /// has had no refinement and [`PROBE_INTERVAL`] has passed since the
    /// last probe. `None` with nothing at this size to judge by.
    fn refine_plan(&self, now: Instant) -> Option<(u8, bool)> {
        let stream = self.target().quality;
        let lowest = stream.saturating_add(REFINE_QUALITY_STEP).min(100);
        let slowest = self
            .viewers
            .iter()
            .filter(|v| !v.stalled)
            .map(|v| v.throughput)
            .try_fold(f64::INFINITY, |slowest, rate| Some(slowest.min(rate?)));
        let budget = slowest.map(|rate| rate * REFINE_LINK_BUDGET.as_secs_f64());
        let steps = (0..).map(|n| 100u8.saturating_sub(n * REFINE_QUALITY_STEP));
        let fitting = steps.take_while(|q| *q > lowest).chain([lowest]).find(|q| {
            let estimate = self.estimate(stream, *q);
            matches!((estimate, budget), (Some(e), Some(b)) if e * self.refine_ratio <= b)
        });
        match fitting {
            Some(quality) if self.refined_at.is_none_or(|at| quality > at) => {
                Some((quality, false))
            }
            Some(_) => None,
            None => {
                let probe = self.last_keyframe.is_some()
                    && self.refined_at.is_none()
                    && self.last_probe.is_none_or(|at| now >= at + PROBE_INTERVAL);
                probe.then_some((lowest, true))
            }
        }
    }

    /// Whether the picture has settled for a refinement at `now`: a lossy
    /// stream, nothing new waiting, every viewer caught up, and the last
    /// frame [`REFINE_AFTER`] old.
    fn settled(&self, now: Instant) -> bool {
        self.refinement
            && self.last.is_some()
            && self.pending.is_none()
            && self.last_stale_since.is_none()
            && self.last_encode.is_some_and(|at| now >= at + REFINE_AFTER)
            && self
                .viewers
                .iter()
                .filter(|v| !v.stalled)
                .all(|v| v.in_flight.is_empty())
    }

    /// The refinement due at `now`, its quality and whether it is a probe.
    fn refine_due(&self, now: Instant) -> Option<(u8, bool)> {
        self.settled(now).then(|| self.refine_plan(now)).flatten()
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
            throughput: None,
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

    /// The stream's encoder was replaced: every viewer needs a keyframe, and
    /// what is on its way will never be acknowledged as part of the stream
    /// the new encoder starts.
    pub fn restart(&mut self) {
        for viewer in &mut self.viewers {
            viewer.needs_keyframe = true;
            viewer.in_flight.clear();
        }
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
        // Frames of the old size say nothing about refinements at the new one.
        self.last_keyframe = None;
        self.size_from = self.next_seq;
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
        let mut target = self.target();
        if self
            .last_encode
            .is_some_and(|last| now < last + target.frame_interval)
        {
            return None;
        }
        let keyframe = self.viewers.iter().any(|v| !v.stalled && v.needs_keyframe);
        let plan = if keyframe { None } else { self.refine_due(now) };
        let frame = match self.pending.take() {
            Some(frame) => frame,
            None if keyframe => self.cached(now)?.clone(),
            None if plan.is_some() => self.last.clone()?,
            None => return None,
        };
        // A keyframe (a viewer joining, say) is coded at the stream's quality
        // and goes to every viewer, so the picture is refined again after it.
        let refine = plan.is_some();
        let mut estimate = None;
        match plan {
            Some((quality, probe)) => {
                estimate = self.estimate(target.quality, quality);
                self.refined_at = Some(quality);
                if probe {
                    self.last_probe = Some(now);
                }
                target.quality = quality;
            }
            None => self.refined_at = None,
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.sent.len() >= KEPT_FRAMES {
            self.sent.pop_front();
        }
        self.sent.push_back(Sent {
            seq,
            quality: target.quality,
            refine,
            estimate,
            size: None,
        });
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
            refine,
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
        let frame = self.sent.iter().find(|s| s.seq == seq).copied();
        let Some(viewer) = self.viewer(id) else {
            return;
        };
        let sent = viewer
            .in_flight
            .iter()
            .find(|(sent, _)| *sent == seq)
            .map(|(_, at)| *at);
        // Only a large frame measures the link: a small one's time is the
        // round trip. The base delay is taken off what remains, so there is
        // no sample before it is known.
        if let (Some(frame), Some(at), Some(base)) = (frame, sent, viewer.rate.base_delay())
            && let Some((bytes, keyframe)) = frame.size
        {
            let delay = now.saturating_duration_since(at).as_secs_f64();
            if keyframe || frame.refine || delay >= LARGE_FRAME_DELAY * base.max(0.001) {
                let sample = bytes as f64 / (delay - base).max(0.001);
                viewer.throughput = Some(average(viewer.throughput, sample, SHORT_WEIGHT));
            }
        }
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
        self.last.as_ref()?;
        if !active().any(|v| v.needs_keyframe) {
            // The refinement, once the picture has been still long enough;
            // an acknowledgement still out makes it wait for that instead.
            let last = self.last_encode?;
            let still = (last + REFINE_AFTER).max(interval);
            if !self.settled(still) {
                return None;
            }
            // A probe that is not due yet is due once its interval passed.
            let probe = self
                .last_probe
                .map_or(still, |at| (at + PROBE_INTERVAL).max(still));
            return [still, probe]
                .into_iter()
                .find(|at| self.refine_plan(*at).is_some());
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

    /// The lowest delay seen over [`BASE_WINDOW`], in seconds.
    fn base_delay(&self) -> Option<f64> {
        self.base.front().map(|(_, delay)| *delay)
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

    /// A lossy stream whose picture stops changing sends the last frame once
    /// more as a refinement, only after every viewer acknowledged it and
    /// [`REFINE_AFTER`] passed; a lossless stream never does.
    #[test]
    fn a_still_picture_is_refined_once_after_it_settles() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        assert!(!first.refine);
        pacer.encoded(first.seq, 10_000, true);
        // Unacknowledged, the frame may still be on its way: no refinement.
        assert_eq!(pacer.wake_at(t0 + REFINE_AFTER), None);
        assert_eq!(pacer.poll(t0 + REFINE_AFTER * 2), None);
        pacer.ack(A, first.seq, t0 + ms(10));
        measured(&mut pacer, A, 1_000_000.0);
        assert_eq!(pacer.wake_at(t0 + ms(20)), Some(t0 + REFINE_AFTER));
        assert_eq!(pacer.poll(t0 + REFINE_AFTER - ms(1)), None);
        let refined = pacer.poll(t0 + REFINE_AFTER).expect("the refinement");
        assert!(refined.refine && !refined.keyframe);
        assert_eq!((refined.frame, refined.target.quality), (1, 100));
        pacer.ack(A, refined.seq, t0 + REFINE_AFTER + ms(5));
        assert_eq!(pacer.wake_at(t0 + REFINE_AFTER * 3), None, "only once");
        assert_eq!(pacer.poll(t0 + REFINE_AFTER * 3), None);

        // A new picture is refined again once it settles.
        let t1 = t0 + REFINE_AFTER * 4;
        let next = step(&mut pacer, 2, t1, &[(A, ms(5))]).expect("new picture");
        assert!(!next.refine);
        let again = pacer.poll(t1 + REFINE_AFTER).expect("refined again");
        assert_eq!((again.frame, again.refine), (2, true));

        let mut lossless = Pacer::new();
        lossless.add_viewer(A);
        step(&mut lossless, 1, t0, &[(A, ms(5))]).expect("frame");
        assert_eq!(lossless.wake_at(t0 + ms(10)), None);
        assert_eq!(lossless.poll(t0 + REFINE_AFTER * 2), None);
    }

    /// Sets `viewer`'s measured throughput to `rate` bytes a second.
    fn measured(pacer: &mut Pacer<u32>, viewer: SessionId, rate: f64) {
        pacer.viewer(viewer).expect("the viewer").throughput = Some(rate);
    }

    /// The refinement after a keyframe of `bytes` at the stream's quality,
    /// `level` setting it (0.0 is quality 20, 0.5 quality 55), on a link
    /// measured at `rate` bytes a second: its quality and whether it is a
    /// probe, or `None` when it is skipped.
    fn refinement_after(level: f64, bytes: usize, rate: f64) -> Option<(u8, bool)> {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        pacer.viewer(A).expect("A").rate.level = level;
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        pacer.encoded(first.seq, bytes, true);
        pacer.ack(A, first.seq, t0 + ms(10));
        pacer.viewer(A).expect("A").rate.level = level;
        measured(&mut pacer, A, rate);
        let at = t0 + REFINE_AFTER;
        let due = pacer.wake_at(at).is_some();
        let refined = pacer.poll(at);
        assert_eq!(due, refined.is_some(), "the timer agrees with poll");
        let probe = pacer.last_probe.is_some();
        refined.map(|r| {
            assert!(r.refine && !r.keyframe);
            (r.target.quality, probe)
        })
    }

    /// The refinement takes the finest quality whose estimate, a keyframe at
    /// the refinement's capped quantizer times what refinements cost against
    /// one, the link carries within [`REFINE_LINK_BUDGET`]; when none fits,
    /// the lowest goes out as a probe. Without anything to judge by there
    /// is no refinement.
    #[test]
    fn a_refinement_takes_the_best_quality_the_link_carries() {
        // A 73 KB keyframe at quality 20 on 1 MB/s (300 KB in the budget):
        // 100 and 80 both code at the capped quantizer, 15 steps below 39,
        // estimated at 124 KB, which fits; quality 60 would not sharpen.
        assert_eq!(refine_qp(100, quantizer(20)), refine_qp(80, quantizer(20)));
        assert_eq!(
            refinement_after(0.0, 73_000, 1_000_000.0),
            Some((100, false))
        );
        // At quality 55: 300 KB would take 360 KB at 100, and 180 KB at 80.
        assert_eq!(
            refinement_after(0.5, 300_000, 1_000_000.0),
            Some((80, false))
        );
        // 200 KB/s: 60 KB in the budget, and quality 75 would take 160 KB.
        assert_eq!(refinement_after(0.5, 300_000, 200_000.0), Some((75, true)));

        let t0 = Instant::now();
        let mut unmeasured = Pacer::new().with_refinement();
        unmeasured.add_viewer(A);
        step(&mut unmeasured, 1, t0, &[(A, ms(10))]).expect("frame");
        assert_eq!(unmeasured.wake_at(t0 + ms(20)), None);
        assert_eq!(
            unmeasured.poll(t0 + REFINE_AFTER * 2),
            None,
            "no size known"
        );
    }

    /// A viewer that joins a refined picture gets its keyframe at the
    /// stream's quality, as does everybody else; the refinement follows under
    /// the same rule once the joiner's link is measured too.
    #[test]
    fn a_joiner_gets_the_stream_quality_and_then_the_refinement() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        pacer.encoded(first.seq, 10_000, true);
        pacer.ack(A, first.seq, t0 + ms(10));
        measured(&mut pacer, A, 1_000_000.0);
        let refined = pacer.poll(t0 + REFINE_AFTER).expect("the refinement");
        pacer.encoded(refined.seq, 30_000, false);
        pacer.ack(A, refined.seq, t0 + REFINE_AFTER + ms(30));

        let t1 = t0 + REFINE_AFTER * 2;
        pacer.add_viewer(B);
        let joined = pacer.poll(t1).expect("the joiner's keyframe");
        assert!(joined.keyframe && !joined.refine, "{joined:?}");
        assert_eq!(joined.target.quality, first.target.quality);
        assert_eq!(joined.recipients, vec![A, B]);
        pacer.encoded(joined.seq, 10_000, true);
        pacer.ack(A, joined.seq, t1 + ms(10));
        pacer.ack(B, joined.seq, t1 + ms(20));
        let again = pacer.poll(t1 + REFINE_AFTER).expect("refined again");
        assert!(again.refine && !again.keyframe);
        assert_eq!(again.recipients, vec![A, B]);
    }

    /// A link too slow for any refinement gets the lowest as a probe at most
    /// every [`PROBE_INTERVAL`]: a picture in between is not refined, and once
    /// a probe measures the link fast, a finer refinement follows.
    #[test]
    fn a_skipped_refinement_comes_back_once_a_probe_measures_the_link_fast() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        pacer.encoded(first.seq, 100_000, true);
        pacer.ack(A, first.seq, t0 + ms(10));
        // 50 KB/s: 15 KB in the budget, and the lowest refinement would take 53 KB.
        measured(&mut pacer, A, 50_000.0);
        let probe = pacer.poll(t0 + REFINE_AFTER).expect("the probe");
        assert_eq!((probe.refine, probe.target.quality), (true, 75));
        pacer.encoded(probe.seq, 50_000, false);
        // The probe took a second: the link is as slow as measured.
        pacer.ack(A, probe.seq, t0 + REFINE_AFTER + ms(1_010));

        // A new picture is not refined before the probe interval passed.
        let t1 = t0 + Duration::from_secs(5);
        step(&mut pacer, 2, t1, &[(A, ms(10))]).expect("new picture");
        assert_eq!(pacer.poll(t1 + REFINE_AFTER), None);
        let due = pacer.last_probe.expect("probed") + PROBE_INTERVAL;
        assert_eq!(pacer.wake_at(t1 + REFINE_AFTER), Some(due));

        // The next probe goes out at the interval and measures the link fast.
        // (The slow acknowledgement lowered the stream's quality, and the
        // probe's with it.)
        let again = pacer.poll(due).expect("the next probe");
        assert!(again.refine && again.target.quality <= 75, "{again:?}");
        pacer.encoded(again.seq, 50_000, false);
        pacer.ack(A, again.seq, due + ms(20));
        let finer = pacer.poll(due + REFINE_AFTER).expect("a finer refinement");
        assert!(
            finer.refine && finer.target.quality > again.target.quality,
            "{finer:?}"
        );
    }

    /// A keyframe of an early, mostly empty picture does not set the
    /// estimate once larger frames filled the window: on a slow link the
    /// refinement stays within what those frames say it would cost.
    #[test]
    fn a_small_early_keyframe_does_not_allow_a_large_refinement() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        pacer.encoded(first.seq, 2_000, true);
        pacer.ack(A, first.seq, t0 + ms(10));
        let mut at = t0;
        for frame in 2..5 {
            at += ms(100);
            let drawn = step(&mut pacer, frame, at, &[(A, ms(10))]).expect("frame");
            pacer.encoded(drawn.seq, 80_000, false);
        }
        pacer.viewer(A).expect("A").rate.level = 0.5;
        // 300 KB/s: 90 KB in the budget. The keyframe alone would allow 100
        // (2.4 KB); the frames after it estimate 100 at 96 KB and 80 at 48 KB.
        measured(&mut pacer, A, 300_000.0);
        let refined = pacer.poll(at + REFINE_AFTER).expect("the refinement");
        assert_eq!((refined.refine, refined.target.quality), (true, 80));
    }

    /// The first keyframe's acknowledgement measures no throughput: before
    /// the base delay is known, its time would count the whole round trip.
    #[test]
    fn no_throughput_is_measured_before_the_base_delay() {
        let t0 = Instant::now();
        let mut pacer = Pacer::new().with_refinement();
        pacer.add_viewer(A);
        let first = step(&mut pacer, 1, t0, &[]).expect("first frame");
        pacer.encoded(first.seq, 100_000, true);
        pacer.ack(A, first.seq, t0 + ms(50));
        assert_eq!(pacer.viewer(A).expect("A").throughput, None);
        pacer.offer(2);
        let key = pacer.poll(t0 + ms(100)).expect("frame");
        pacer.encoded(key.seq, 100_000, true);
        pacer.ack(A, key.seq, t0 + ms(200));
        let rate = pacer.viewer(A).expect("A").throughput.expect("measured");
        // 100 KB in the 50 ms beyond the 50 ms base delay.
        assert!((rate - 2_000_000.0).abs() < 1.0, "{rate}");
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
