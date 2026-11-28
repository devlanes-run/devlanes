//! The client against a host scripted by hand over an in-memory pipe, for
//! what a real host cannot be made to do: send frames that do not decode,
//! speak another version, or watch keep-alives closely.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use devlanes_gui_client::{Client, ClientError, ClientOptions, Event, Events, connect_stream};
use devlanes_gui_codec::{EncodeParams, Frame, RawEncoder, VideoEncoder};
use devlanes_gui_proto::{
    Buttons, ClientMessage, CodecId, DecodeError, HEADER_LEN, HEARTBEAT_INTERVAL, Hello,
    HostMessage, LockState, MAGIC, Point, Pointer, Role, SYNC_FEATURE, SessionId, Size, VideoFrame,
    Welcome, WindowId, WindowInfo, WindowKind,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

const W: WindowId = WindowId(1);

/// Longest a test waits; on the paused clock a hang fails at once.
const PATIENCE: Duration = Duration::from_secs(60);

/// The host end of a pipe, spoken by hand.
struct Script {
    stream: DuplexStream,
    buf: BytesMut,
}

impl Script {
    /// The next message from the client.
    async fn read(&mut self) -> ClientMessage {
        let wait = async {
            loop {
                if let Some(message) = ClientMessage::decode(&mut self.buf).expect("a message") {
                    return message;
                }
                let read = self.stream.read_buf(&mut self.buf).await.expect("reads");
                assert!(read > 0, "the client closed the connection");
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .expect("the client sent something")
    }

    /// Writes `message`.
    async fn send(&mut self, message: &HostMessage) {
        let mut out = BytesMut::new();
        message.encode(&mut out).expect("encodes");
        self.stream.write_all(&out).await.expect("writes");
    }
}

/// The welcome of a host with one window.
fn welcome() -> HostMessage {
    HostMessage::Welcome(Welcome {
        session: SessionId(7),
        service: "acme:desktop".into(),
        encoders: Vec::new(),
        real_desktop: false,
        windows: vec![WindowInfo {
            id: W,
            title: "Acme".into(),
            app_id: String::new(),
            size: Size::new(8, 8),
            min_size: None,
            max_size: None,
            draws_own_frame: false,
            reports_moves: false,
            parent: None,
            offset: Point::default(),
            lock: LockState::Unlocked,
            kind: WindowKind::Toplevel,
        }],
        features: vec![SYNC_FEATURE.into()],
    })
}

/// A client with `role` connected to a script that answers its hello with `answer`.
async fn connect(role: Role, answer: Bytes) -> (Result<(Client, Events), ClientError>, Script) {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let mut script = Script {
        stream: theirs,
        buf: BytesMut::new(),
    };
    let client = tokio::spawn(connect_stream(ours, ClientOptions::new(role, "ada", "ada")));
    let hello = async {
        loop {
            if let Some(hello) = ClientMessage::decode_hello(&mut script.buf).expect("a hello") {
                return hello;
            }
            script
                .stream
                .read_buf(&mut script.buf)
                .await
                .expect("reads");
        }
    };
    let _: Hello = tokio::time::timeout(PATIENCE, hello)
        .await
        .expect("a hello");
    script.stream.write_all(&answer).await.expect("writes");
    let client = client.await.expect("no panic");
    (client, script)
}

/// `message` encoded.
fn bytes(message: &HostMessage) -> Bytes {
    let mut out = BytesMut::new();
    message.encode(&mut out).expect("encodes");
    out.freeze()
}

/// A raw frame of `window` with sequence number `seq`.
fn raw_frame(seq: u64, payload: Bytes) -> HostMessage {
    HostMessage::VideoFrame(VideoFrame {
        window: W,
        seq,
        codec: CodecId::RAW,
        keyframe: true,
        size: Size::new(8, 8),
        timestamp_us: 0,
        payload,
    })
}

#[tokio::test(start_paused = true)]
async fn a_frame_that_fails_to_decode_is_acknowledged_and_a_keyframe_requested() {
    let (client, mut host) = connect(Role::Viewer, bytes(&welcome())).await;
    let (_client, mut events) = client.expect("welcomed");
    let picture = Frame::packed(8, 8, vec![0x5a; 8 * 8 * 4]).expect("a frame");
    let params = EncodeParams {
        bitrate_bps: 0,
        frame_interval: Duration::ZERO,
        quality: 100,
        keyframe: true,
    };
    let good = RawEncoder::new()
        .encode(&picture, &params)
        .expect("encodes")
        .expect("a keyframe");

    host.send(&raw_frame(1, Bytes::from_static(b"not zstd")))
        .await;
    host.send(&raw_frame(2, good.payload)).await;
    assert_eq!(
        host.read().await,
        ClientMessage::FrameAck { window: W, seq: 1 }
    );
    assert_eq!(
        host.read().await,
        ClientMessage::KeyframeRequest { window: W }
    );

    // The broken frame never reaches the caller; the next one does, and is
    // acknowledged as it is read.
    let frame = tokio::time::timeout(PATIENCE, events.next())
        .await
        .expect("an event");
    let Some(Event::Frame(frame)) = frame else {
        panic!("expected a frame, got {frame:?}");
    };
    assert_eq!(
        (frame.seq, frame.frame.to_packed()),
        (2, picture.to_packed())
    );
    assert_eq!(
        host.read().await,
        ClientMessage::FrameAck { window: W, seq: 2 }
    );
}

#[tokio::test(start_paused = true)]
async fn keep_alives_go_out_only_while_input_is_held() {
    let (client, mut host) = connect(Role::Tool, bytes(&welcome())).await;
    let (client, _events) = client.expect("welcomed");
    let press = |buttons| Pointer {
        window: W,
        position: Point::new(1, 1),
        buttons,
        screen_delta: None,
    };

    client.pointer(press(Buttons::LEFT)).expect("sends");
    assert!(matches!(host.read().await, ClientMessage::Pointer(_)));
    let pressed = tokio::time::Instant::now();
    for _ in 0..3 {
        assert_eq!(host.read().await, ClientMessage::KeepAlive);
    }
    assert!(pressed.elapsed() <= 3 * HEARTBEAT_INTERVAL);

    client.pointer(press(Buttons::NONE)).expect("sends");
    assert!(matches!(host.read().await, ClientMessage::Pointer(_)));
    tokio::time::sleep(5 * HEARTBEAT_INTERVAL).await;
    // Nothing came in between: the next message is the one sent now.
    client.text(W, "done").expect("sends");
    assert!(matches!(host.read().await, ClientMessage::Text { .. }));
}

#[tokio::test(start_paused = true)]
async fn a_host_of_another_version_or_protocol_is_a_typed_error() {
    let mut newer = BytesMut::from(&bytes(&welcome())[..]);
    let at = HEADER_LEN + MAGIC.len();
    newer[at..at + 2].copy_from_slice(&9u16.to_be_bytes());
    let (client, _) = connect(Role::Viewer, newer.freeze()).await;
    assert!(
        matches!(
            client,
            Err(ClientError::Decode(DecodeError::VersionMismatch {
                theirs: 9,
                ..
            }))
        ),
        "{:?}",
        client.err()
    );

    let http = Bytes::from_static(b"HTTP/1.1 400 Bad Request\r\n\r\n");
    let (client, _) = connect(Role::Tool, http).await;
    assert!(
        matches!(client, Err(ClientError::Decode(DecodeError::BadMagic))),
        "{:?}",
        client.err()
    );
}
