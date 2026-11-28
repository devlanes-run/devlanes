//! Video streams: pacing between viewers, keyframes for late joiners, one
//! encoder per codec, and screenshots.

use bytes::Bytes;
use devlanes_gui_client::ClientError;
use devlanes_gui_codec::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, RawBackend,
    VideoDecoder, VideoEncoder,
};
use devlanes_gui_host::{Command, MAX_SCREENSHOTS_IN_FLIGHT};
use devlanes_gui_host::pattern::shows_grid;
use devlanes_gui_proto::{MAX_IN_FLIGHT, Point, Size, WindowId};

use super::*;

/// Frames that arrive within `during`, pausing `pause` after each before it
/// is acknowledged. Returns their count and the last sequence number.
async fn count_frames(events: &mut Events, during: Duration, pause: Duration) -> (usize, u64) {
    let deadline = tokio::time::Instant::now() + during;
    let (mut count, mut last) = (0, 0);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let Event::Frame(frame) = event {
            count += 1;
            last = frame.seq;
            tokio::time::sleep(pause).await;
        }
    }
    (count, last)
}

/// Redraws `window` every 20 ms for `during`, as an animation would.
fn animate(host: &Host, window: WindowId, during: Duration) {
    let pattern = host.pattern.clone();
    tokio::spawn(async move {
        let end = tokio::time::Instant::now() + during;
        while tokio::time::Instant::now() < end {
            pattern.redraw(window);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

#[tokio::test(start_paused = true)]
async fn the_slowest_viewer_sets_the_pace_of_a_shared_stream() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (_ada, mut fast) = host.viewer("ada").await;
    let (_bob, mut slow) = host.viewer("bob").await;
    let during = Duration::from_secs(3);
    animate(&host, window, during);
    let ((fast_frames, _), (slow_frames, _)) = tokio::join!(
        count_frames(&mut fast, during, Duration::ZERO),
        count_frames(&mut slow, during, Duration::from_millis(250)),
    );
    // 150 redraws; alone, ada would get one per frame interval.
    assert!(fast_frames > MAX_IN_FLIGHT, "{fast_frames}");
    assert!(
        fast_frames <= slow_frames + MAX_IN_FLIGHT,
        "ada got {fast_frames}, bob finished {slow_frames}"
    );
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_viewer_that_stops_acknowledging_is_set_aside_and_rejoins_once_caught_up() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (_ada, mut ada_events) = host.viewer("ada").await;
    let (_bob, mut bob_events) = host.viewer("bob").await;
    let during = Duration::from_secs(4);
    animate(&host, window, during);
    // Bob reads nothing: it holds the stream back only until it is set aside.
    let (frames, ada_last) = count_frames(&mut ada_events, during, Duration::ZERO).await;
    assert!(frames > 4 * MAX_IN_FLIGHT, "ada got only {frames} frames");

    // Bob works through what it got, and a frame newer than all of ada's follows.
    let mut seen = Vec::new();
    while seen.len() < MAX_IN_FLIGHT {
        seen.push(next_frame(&mut bob_events).await.seq);
    }
    assert!(seen.iter().all(|seq| *seq < ada_last), "{seen:?}");
    let rejoin = next_frame(&mut bob_events).await;
    assert!(rejoin.seq > ada_last, "{} after {ada_last}", rejoin.seq);
    assert!(shows_grid(&rejoin.frame));
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_viewer_joining_a_static_window_gets_the_cached_frame() {
    let options = TestPatternOptions {
        changes_only: true,
        ..TestPatternOptions::default()
    };
    let host = Host::start(options, |_| {}).await;
    let (_ada, mut ada_events) = host.viewer("ada").await;
    let first = next_frame(&mut ada_events).await;
    tokio::time::sleep(Duration::from_secs(30)).await;

    let (_bob, mut bob_events) = host.viewer("bob").await;
    let joined = next_frame(&mut bob_events).await;
    assert!(joined.keyframe);
    assert_eq!(joined.frame.to_packed(), first.frame.to_packed());
    let captured_again = host
        .pattern
        .commands()
        .iter()
        .any(|c| matches!(c, Command::RequestFrame { .. }));
    assert!(!captured_again, "the picture came from the host's cache");
    host.stop().await;
}

/// Counts the encoders a backend makes.
struct Counting {
    inner: Arc<dyn EncoderBackend>,
    made: Arc<AtomicUsize>,
}

impl EncoderBackend for Counting {
    /// The wrapped backend's offer.
    fn offer(&self) -> CodecOffer {
        self.inner.offer()
    }

    /// Counts, then asks the wrapped backend.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        self.made.fetch_add(1, Ordering::SeqCst);
        self.inner.encoder()
    }
}

/// Stands in for H.264: uncompressed pixels under the H.264 codec id.
const PLAIN: CodecOffer = CodecOffer {
    codec: CodecId::H264,
    hardware: false,
};

/// An encoder of [`PLAIN`] and the backend that makes it.
struct Plain;

impl EncoderBackend for Plain {
    /// [`PLAIN`].
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    /// A [`Plain`] encoder.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Ok(Box::new(Plain))
    }
}

impl VideoEncoder for Plain {
    /// [`PLAIN`].
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    /// The packed pixels, every frame a keyframe.
    fn encode(&mut self, frame: &Frame, _: &EncodeParams) -> Result<Option<Encoded>, CodecError> {
        Ok(Some(Encoded {
            payload: frame.to_packed(),
            keyframe: true,
            width: frame.width(),
            height: frame.height(),
        }))
    }
}

/// A backend of [`PLAIN`] whose encoder never starts, like a hardware
/// encoder whose device is gone.
struct Broken;

impl EncoderBackend for Broken {
    /// [`PLAIN`].
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    /// Always fails.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Err(CodecError::Unavailable {
            codec: "h264",
            reason: "no encoder device".into(),
        })
    }
}

/// Decodes [`Plain`] and counts what it decoded.
struct PlainDecoder(Arc<AtomicUsize>);

impl DecoderBackend for PlainDecoder {
    /// [`PLAIN`].
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    /// A decoder sharing this one's count.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(PlainDecoder(self.0.clone())))
    }
}

impl VideoDecoder for PlainDecoder {
    /// [`PLAIN`].
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    /// Counts the packet and takes its bytes as packed pixels.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let pixels = Bytes::copy_from_slice(packet.payload);
        Frame::packed(packet.width, packet.height, pixels).map(Some)
    }
}

#[tokio::test(start_paused = true)]
async fn viewers_of_another_codec_get_an_encoder_of_their_own() {
    let raw_made = Arc::new(AtomicUsize::new(0));
    let plain_made = Arc::new(AtomicUsize::new(0));
    let encoders: Vec<Arc<dyn EncoderBackend>> = vec![
        Arc::new(Counting {
            inner: Arc::new(RawBackend),
            made: raw_made.clone(),
        }),
        Arc::new(Counting {
            inner: Arc::new(Plain),
            made: plain_made.clone(),
        }),
    ];
    let host = Host::start(TestPatternOptions::default(), |o| o.encoders = encoders).await;
    let decoded = Arc::new(AtomicUsize::new(0));
    let (_ada, mut ada) = host.viewer("ada").await;
    let (_cy, mut cy) = host.viewer("cy").await;
    let plain = ClientOptions::new(Role::Viewer, "bob", "bob")
        .with_decoders(vec![Arc::new(PlainDecoder(decoded.clone()))]);
    let (_bob, mut bob) = host.connect(plain).await;
    for events in [&mut ada, &mut cy, &mut bob] {
        let frame = next_frame(events).await;
        assert!(shows_grid(&frame.frame));
    }
    assert_eq!(raw_made.load(Ordering::SeqCst), 1, "ada and cy share one");
    assert_eq!(plain_made.load(Ordering::SeqCst), 1);
    assert!(
        decoded.load(Ordering::SeqCst) >= 1,
        "bob's frames were plain"
    );
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn viewers_of_an_encoder_that_cannot_start_fall_back_to_another_codec() {
    let encoders: Vec<Arc<dyn EncoderBackend>> = vec![Arc::new(Broken), Arc::new(RawBackend)];
    let host = Host::start(TestPatternOptions::default(), |o| o.encoders = encoders).await;
    let decoded = Arc::new(AtomicUsize::new(0));
    let both = ClientOptions::new(Role::Viewer, "ada", "ada").with_decoders(vec![
        Arc::new(PlainDecoder(decoded.clone())),
        Arc::new(RawBackend),
    ]);
    let (_ada, mut events) = host.connect(both).await;
    let frame = next_frame(&mut events).await;
    assert!(shows_grid(&frame.frame));
    assert_eq!(decoded.load(Ordering::SeqCst), 0, "the frame came as raw");

    // A viewer joining later gets raw straight away.
    let both = ClientOptions::new(Role::Viewer, "bob", "bob").with_decoders(vec![
        Arc::new(PlainDecoder(decoded.clone())),
        Arc::new(RawBackend),
    ]);
    let (_bob, mut events) = host.connect(both).await;
    assert!(shows_grid(&next_frame(&mut events).await.frame));
    assert_eq!(decoded.load(Ordering::SeqCst), 0);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_backend_is_told_which_windows_are_watched() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let watches = |host: &Host| {
        host.pattern
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Watch { window, watched } => Some((window, watched)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let (ada, mut ada_events) = host.viewer("ada").await;
    let (bob, _bob_events) = host.viewer("bob").await;
    next_frame(&mut ada_events).await;
    assert_eq!(watches(&host), vec![(window, true)], "once for both viewers");
    ada.close();
    bob.close();
    let (tool, _events) = host.tool("agent").await;
    eventually("the last viewer is gone", || {
        watches(&host) == vec![(window, true), (window, false)]
    })
    .await;

    // Nobody watches, so a screenshot asks for a fresh frame instead of the cache.
    let before = host.pattern.commands().len();
    tool.screenshot(Some(window)).await.expect("a screenshot");
    let asked = host.pattern.commands()[before..]
        .iter()
        .any(|c| matches!(c, Command::RequestFrame { .. }));
    assert!(asked);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_tool_with_more_screenshots_than_the_host_allows_waits_its_turn() {
    let host = Host::default().await;
    let (tool, _events) = host.tool("agent").await;
    // Unwatched, so each screenshot waits for a frame the pattern holds back.
    host.pattern.hold_frames(true);
    let mut shots = tokio::task::JoinSet::new();
    for _ in 0..2 * MAX_SCREENSHOTS_IN_FLIGHT {
        let tool = tool.clone();
        shots.spawn(async move { tool.screenshot(None).await });
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(shots.try_join_next().is_none(), "they wait for frames");
    host.pattern.hold_frames(false);
    let mut taken = 0;
    while let Some(shot) = shots.join_next().await {
        let shot = shot.expect("no panic").expect("a screenshot");
        assert!(shows_grid(&shot.frame));
        taken += 1;
    }
    assert_eq!(taken, 2 * MAX_SCREENSHOTS_IN_FLIGHT);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_keyframe_request_is_answered_from_the_cache() {
    let options = TestPatternOptions {
        changes_only: true,
        ..TestPatternOptions::default()
    };
    let host = Host::start(options, |_| {}).await;
    let window = host.pattern.first_window();
    let (ada, mut events) = host.viewer("ada").await;
    let first = next_frame(&mut events).await;
    ada.request_keyframe(window).expect("sends");
    let again = next_frame(&mut events).await;
    assert!(again.seq > first.seq && again.keyframe);
    assert_eq!(again.frame.to_packed(), first.frame.to_packed());
    let captured = host
        .pattern
        .commands()
        .iter()
        .any(|c| matches!(c, Command::RequestFrame { .. }));
    assert!(!captured, "no new capture was needed");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_screenshot_shows_one_window_and_names_a_missing_one() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (tool, _events) = host.tool("agent").await;
    let popup = host
        .pattern
        .open_popup(window, Point::new(20, 30), Size::new(80, 40));
    // The popup's first frame was sent before the frame this waits for.
    tool.sync().await.expect("synced");

    let main = tool.screenshot(None).await.expect("a screenshot");
    assert_eq!(main.window, window);
    assert_eq!((main.frame.width(), main.frame.height()), (320, 200));
    assert!(shows_grid(&main.frame));
    let shot = tool.screenshot(Some(popup)).await.expect("a screenshot");
    assert_eq!((shot.frame.width(), shot.frame.height()), (80, 40));
    let missing = tool.screenshot(Some(WindowId(99))).await;
    assert!(
        matches!(missing, Err(ClientError::NoSuchWindow)),
        "{missing:?}"
    );
    host.stop().await;
}
