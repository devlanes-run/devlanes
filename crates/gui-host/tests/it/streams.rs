//! Video streams: pacing between viewers, keyframes for late joiners, one
//! encoder per codec, and screenshots.

use bytes::Bytes;
use devlanes_gui_client::ClientError;
use devlanes_gui_codec::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, RawBackend,
    VideoDecoder, VideoEncoder,
};
use devlanes_gui_host::Command;
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
    let host = Host::default();
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
    let host = Host::default();
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
    let host = Host::start(options, |_| {});
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
    fn offer(&self) -> CodecOffer {
        self.inner.offer()
    }

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

struct Plain;

impl EncoderBackend for Plain {
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Ok(Box::new(Plain))
    }
}

impl VideoEncoder for Plain {
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    fn encode(&mut self, frame: &Frame, _: &EncodeParams) -> Result<Option<Encoded>, CodecError> {
        Ok(Some(Encoded {
            payload: frame.to_packed(),
            keyframe: true,
            width: frame.width(),
            height: frame.height(),
        }))
    }
}

/// Decodes [`Plain`] and counts what it decoded.
struct PlainDecoder(Arc<AtomicUsize>);

impl DecoderBackend for PlainDecoder {
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(PlainDecoder(self.0.clone())))
    }
}

impl VideoDecoder for PlainDecoder {
    fn offer(&self) -> CodecOffer {
        PLAIN
    }

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
    let host = Host::start(TestPatternOptions::default(), |o| o.encoders = encoders);
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
    assert!(decoded.load(Ordering::SeqCst) >= 1, "bob's frames were plain");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_screenshot_shows_one_window_and_names_a_missing_one() {
    let host = Host::default();
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
    assert!(matches!(missing, Err(ClientError::NoSuchWindow)), "{missing:?}");
    host.stop().await;
}
