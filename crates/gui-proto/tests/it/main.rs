//! The wire format through the public API: round trips, chunked input,
//! malformed and oversized frames, compatibility rules.

use bytes::{Bytes, BytesMut};
use devlanes_gui_proto::{
    Buttons, ClientMessage, CodecId, CodecOffer, Cursor, CursorImage, CursorShape, DecodeError,
    Edge, EncodeError, HEADER_LEN, Hello, Holder, HostMessage, Key, KeyCode, LockState,
    MAX_FRAME_PAYLOAD, MAX_TITLE, MAX_UNKNOWN_BODY, MoveResize, PROTOCOL_VERSION, Point, Pointer,
    Refusal, RefusalReason, Role, Screenshot, ScreenshotResult, Size, VideoFrame, Welcome, Wheel,
    WheelUnit, WindowId, WindowInfo,
};

const W: WindowId = WindowId(7);

/// A popup with every optional field set.
fn window() -> WindowInfo {
    WindowInfo {
        id: W,
        title: "Acme: résumé".into(),
        app_id: "run.example.acme".into(),
        size: Size::new(1440, 900),
        min_size: Some(Size::new(400, 300)),
        max_size: None,
        draws_own_frame: true,
        reports_moves: true,
        parent: Some(WindowId(3)),
        offset: Point::new(-12, 40),
        lock: LockState::Resizing,
    }
}

/// One of every client message, with values that use each field's range.
fn client_messages() -> Vec<ClientMessage> {
    let all = vec![
        ClientMessage::Hello(Hello {
            role: Role::Control,
            client_id: "box-1:4242".into(),
            client_name: "ada on acme".into(),
            decoders: vec![
                CodecOffer {
                    codec: CodecId::H264,
                    hardware: true,
                },
                CodecOffer {
                    codec: CodecId(200),
                    hardware: false,
                },
            ],
            features: vec!["later".into()],
            token: Some("0123abcd".into()),
        }),
        ClientMessage::FrameAck {
            window: W,
            seq: u64::MAX,
        },
        ClientMessage::KeyframeRequest { window: W },
        ClientMessage::Pointer(Pointer {
            window: W,
            position: Point::new(-5, i32::MAX),
            buttons: Buttons::LEFT | Buttons::FORWARD,
            screen_delta: Some(Point::new(3, -4)),
        }),
        ClientMessage::Wheel(Wheel {
            window: W,
            position: Point::new(10, 20),
            delta_x: -0.25,
            delta_y: 120.5,
            unit: WheelUnit::Lines,
        }),
        ClientMessage::Key(Key {
            window: W,
            code: KeyCode(0xe1),
            down: true,
        }),
        ClientMessage::Text {
            window: W,
            text: "héllo 🦀".into(),
        },
        ClientMessage::Resize {
            window: W,
            size: Size::new(1200, 700),
        },
        ClientMessage::Close { window: W },
        ClientMessage::Focus {
            window: W,
            focused: true,
        },
        ClientMessage::Clipboard {
            text: "copied".into(),
        },
        ClientMessage::Screenshot {
            request: 9,
            window: Some(W),
        },
        ClientMessage::UnitProcess {
            pid: 31337,
            job: Some("devlanes-acme-web".into()),
        },
        ClientMessage::UnitExited { code: Some(-1) },
        ClientMessage::LaneFrozen,
        ClientMessage::LaneThawed,
    ];
    // Fails to compile when a variant is added, so this list grows with it.
    let mut seen = [false; 16];
    for message in &all {
        let index = match message {
            ClientMessage::Hello(_) => 0,
            ClientMessage::FrameAck { .. } => 1,
            ClientMessage::KeyframeRequest { .. } => 2,
            ClientMessage::Pointer(_) => 3,
            ClientMessage::Wheel(_) => 4,
            ClientMessage::Key(_) => 5,
            ClientMessage::Text { .. } => 6,
            ClientMessage::Resize { .. } => 7,
            ClientMessage::Close { .. } => 8,
            ClientMessage::Focus { .. } => 9,
            ClientMessage::Clipboard { .. } => 10,
            ClientMessage::Screenshot { .. } => 11,
            ClientMessage::UnitProcess { .. } => 12,
            ClientMessage::UnitExited { .. } => 13,
            ClientMessage::LaneFrozen => 14,
            ClientMessage::LaneThawed => 15,
        };
        seen[index] = true;
    }
    assert!(
        seen.iter().all(|s| *s),
        "every client message is in the list"
    );
    all
}

/// One of every host message.
fn host_messages() -> Vec<HostMessage> {
    let all = vec![
        HostMessage::Welcome(Welcome {
            service: "acme:desktop".into(),
            encoders: vec![CodecOffer {
                codec: CodecId::RAW,
                hardware: false,
            }],
            real_desktop: true,
            windows: vec![
                window(),
                WindowInfo {
                    id: WindowId(8),
                    ..window()
                },
            ],
            features: Vec::new(),
        }),
        HostMessage::Refused(Refusal {
            reason: RefusalReason::BadToken,
            message: "wrong token".into(),
        }),
        HostMessage::WindowAdded(window()),
        HostMessage::WindowChanged(WindowInfo {
            min_size: None,
            max_size: Some(Size::new(8192, 8192)),
            parent: None,
            lock: LockState::Frozen,
            ..window()
        }),
        HostMessage::WindowRemoved { window: W },
        HostMessage::VideoFrame(VideoFrame {
            window: W,
            seq: 3,
            codec: CodecId::H264,
            keyframe: true,
            size: Size::new(2, 2),
            timestamp_us: 1_000_000,
            payload: Bytes::from_static(&[0, 0, 0, 1, 0x67, 0x42]),
        }),
        HostMessage::Cursor(Cursor {
            window: W,
            shape: CursorShape::Image(CursorImage {
                size: Size::new(2, 1),
                hotspot: Point::new(1, 0),
                pixels: Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8]),
            }),
            position: Some(Point::new(100, 50)),
        }),
        HostMessage::MoveResizeStarted(MoveResize {
            window: W,
            edge: Some(Edge::BottomRight),
            grab: Point::new(1439, 899),
        }),
        HostMessage::LockChanged {
            window: W,
            state: LockState::Waiting,
        },
        HostMessage::Clipboard {
            text: "from the app".into(),
        },
        HostMessage::InputHolder(Some(Holder {
            client_id: "box-1:4242".into(),
            name: "ada on acme".into(),
        })),
        HostMessage::Screenshot {
            request: 9,
            result: ScreenshotResult::Image(Screenshot {
                window: W,
                size: Size::new(1, 1),
                codec: CodecId::RAW,
                payload: Bytes::from_static(b"zstd"),
            }),
        },
        HostMessage::Heartbeat,
        HostMessage::Activity,
    ];
    // Fails to compile when a variant is added, so this list grows with it.
    let mut seen = [false; 14];
    for message in &all {
        let index = match message {
            HostMessage::Welcome(_) => 0,
            HostMessage::Refused(_) => 1,
            HostMessage::WindowAdded(_) => 2,
            HostMessage::WindowChanged(_) => 3,
            HostMessage::WindowRemoved { .. } => 4,
            HostMessage::VideoFrame(_) => 5,
            HostMessage::Cursor(_) => 6,
            HostMessage::MoveResizeStarted(_) => 7,
            HostMessage::LockChanged { .. } => 8,
            HostMessage::Clipboard { .. } => 9,
            HostMessage::InputHolder(_) => 10,
            HostMessage::Screenshot { .. } => 11,
            HostMessage::Heartbeat => 12,
            HostMessage::Activity => 13,
        };
        seen[index] = true;
    }
    assert!(seen.iter().all(|s| *s), "every host message is in the list");
    all
}

/// Variants of fields with several shapes that the lists above do not use.
fn more_host_messages() -> Vec<HostMessage> {
    let mut all: Vec<HostMessage> = [
        CursorShape::Default,
        CursorShape::Text,
        CursorShape::NwseResize,
        CursorShape::Hidden,
    ]
    .into_iter()
    .map(|shape| {
        HostMessage::Cursor(Cursor {
            window: W,
            shape,
            position: None,
        })
    })
    .collect();
    all.extend(
        [
            ScreenshotResult::NoSuchWindow,
            ScreenshotResult::NoFrame,
            ScreenshotResult::Failed("capture failed".into()),
        ]
        .into_iter()
        .map(|result| HostMessage::Screenshot { request: 1, result }),
    );
    all.push(HostMessage::MoveResizeStarted(MoveResize {
        window: W,
        edge: None,
        grab: Point::new(4, 4),
    }));
    all.push(HostMessage::InputHolder(None));
    all
}

/// The messages as one stream.
fn encode_client(messages: &[ClientMessage]) -> BytesMut {
    let mut out = BytesMut::new();
    for message in messages {
        message.encode(&mut out).expect("encodes");
    }
    out
}

/// The messages as one stream.
fn encode_host(messages: &[HostMessage]) -> BytesMut {
    let mut out = BytesMut::new();
    for message in messages {
        message.encode(&mut out).expect("encodes");
    }
    out
}

/// Feeds `stream` to a decoder in chunks of the given sizes, the way a socket
/// delivers it, and returns every message decoded.
fn decode_in_chunks<M>(
    stream: &[u8],
    chunks: impl IntoIterator<Item = usize>,
    decode: fn(&mut BytesMut) -> Result<Option<M>, DecodeError>,
) -> Vec<M> {
    let mut buf = BytesMut::new();
    let mut out = Vec::new();
    let mut rest = stream;
    for size in chunks.into_iter().chain(std::iter::once(usize::MAX)) {
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        rest = tail;
        buf.extend_from_slice(chunk);
        while let Some(message) = decode(&mut buf).expect("decodes") {
            out.push(message);
        }
        if rest.is_empty() {
            break;
        }
    }
    assert!(buf.is_empty(), "nothing left over");
    out
}

#[test]
fn every_message_survives_a_round_trip_whatever_the_chunking() {
    let client = client_messages();
    let mut host = host_messages();
    host.extend(more_host_messages());
    let client_stream = encode_client(&client);
    let host_stream = encode_host(&host);

    assert_eq!(
        decode_in_chunks(&client_stream, [], ClientMessage::decode),
        client
    );
    assert_eq!(
        decode_in_chunks(&host_stream, [], HostMessage::decode),
        host
    );

    let ones = || std::iter::repeat_n(1, usize::MAX);
    assert_eq!(
        decode_in_chunks(&client_stream, ones(), ClientMessage::decode),
        client
    );
    assert_eq!(
        decode_in_chunks(&host_stream, ones(), HostMessage::decode),
        host
    );

    for split in 0..client_stream.len() {
        assert_eq!(
            decode_in_chunks(&client_stream, [split], ClientMessage::decode),
            client
        );
    }
    for split in 0..host_stream.len() {
        assert_eq!(
            decode_in_chunks(&host_stream, [split], HostMessage::decode),
            host
        );
    }
}

/// Rewrites the body length in the header of a single encoded frame.
fn with_len(frame: &[u8], len: usize) -> BytesMut {
    let mut out = BytesMut::from(&frame[..HEADER_LEN]);
    out[1..HEADER_LEN].copy_from_slice(&(len as u32).to_be_bytes());
    out.extend_from_slice(&frame[HEADER_LEN..HEADER_LEN + len]);
    out
}

#[test]
fn a_body_cut_short_anywhere_is_an_error_not_a_panic() {
    for message in client_messages() {
        let frame = encode_client(std::slice::from_ref(&message));
        for len in 0..frame.len() - HEADER_LEN {
            let result = ClientMessage::decode(&mut with_len(&frame, len));
            assert!(
                result.is_err(),
                "{message:?} cut to {len} bytes gave {result:?}"
            );
        }
    }
    let mut host = host_messages();
    host.extend(more_host_messages());
    for message in host {
        let frame = encode_host(std::slice::from_ref(&message));
        for len in 0..frame.len() - HEADER_LEN {
            let result = HostMessage::decode(&mut with_len(&frame, len));
            assert!(
                result.is_err(),
                "{message:?} cut to {len} bytes gave {result:?}"
            );
        }
    }
}

#[test]
fn random_bodies_never_panic() {
    // A fixed xorshift, so a failure reproduces.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..20_000 {
        let kind = (next() % 0xc0) as u8;
        let len = (next() % 96) as usize;
        let mut frame = BytesMut::new();
        frame.extend_from_slice(&[kind]);
        frame.extend_from_slice(&(len as u32).to_be_bytes());
        frame.extend((0..len).map(|_| next() as u8));
        let result = match kind < 0x80 {
            true => ClientMessage::decode(&mut frame).map(|_| ()),
            false => HostMessage::decode(&mut frame).map(|_| ()),
        };
        // Errors are fine; only reaching here without a panic matters.
        drop(result);
    }
}

#[test]
fn an_oversized_frame_is_refused_from_its_header_alone() {
    let mut buf = BytesMut::from(&[0x04, 0xff, 0xff, 0xff, 0xff][..]);
    assert!(matches!(
        ClientMessage::decode(&mut buf),
        Err(DecodeError::TooLarge {
            kind: 0x04,
            len: u32::MAX,
            ..
        })
    ));

    // A video frame may be large, but not past the payload limit.
    let len = (MAX_FRAME_PAYLOAD + 64 * 1024) as u32;
    let mut buf = BytesMut::from(&[0x86][..]);
    buf.extend_from_slice(&len.to_be_bytes());
    assert!(matches!(
        HostMessage::decode(&mut buf),
        Err(DecodeError::TooLarge { .. })
    ));

    // Unknown types are skipped, but only up to their own limit.
    let mut buf = BytesMut::from(&[0x7f][..]);
    buf.extend_from_slice(&(MAX_UNKNOWN_BODY + 1).to_be_bytes());
    assert!(matches!(
        ClientMessage::decode(&mut buf),
        Err(DecodeError::TooLarge { .. })
    ));
}

#[test]
fn a_list_count_above_its_limit_is_refused_before_reading_entries() {
    let mut frame = encode_host(&[HostMessage::Welcome(Welcome {
        service: String::new(),
        encoders: Vec::new(),
        real_desktop: false,
        windows: Vec::new(),
        features: Vec::new(),
    })]);
    // magic (4), version (2), service length (4), then the encoder count.
    let count = HEADER_LEN + 10;
    frame[count..count + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(
        HostMessage::decode(&mut frame),
        Err(DecodeError::FieldTooLong {
            field: "encoders",
            ..
        })
    ));
}

#[test]
fn unknown_types_and_appended_fields_are_skipped() {
    let mut stream = BytesMut::new();
    stream.extend_from_slice(&[0x7e, 0, 0, 0, 3, 1, 2, 3]);
    let ack = ClientMessage::FrameAck { window: W, seq: 5 };
    let encoded = encode_client(std::slice::from_ref(&ack));
    // The same message from a later version with two more fields at the end.
    let len = encoded.len() - HEADER_LEN + 2;
    stream.extend_from_slice(&encoded[..1]);
    stream.extend_from_slice(&(len as u32).to_be_bytes());
    stream.extend_from_slice(&encoded[HEADER_LEN..]);
    stream.extend_from_slice(&[9, 9]);
    assert_eq!(ClientMessage::decode(&mut stream), Ok(Some(ack)));
    assert!(stream.is_empty());

    // Unknown codecs and cursor names are kept or fall back.
    let mut frame = encode_host(&[HostMessage::Cursor(Cursor {
        window: W,
        shape: CursorShape::Text,
        position: None,
    })]);
    frame[HEADER_LEN + 4] = 0x80;
    let Ok(Some(HostMessage::Cursor(cursor))) = HostMessage::decode(&mut frame) else {
        panic!("cursor decodes");
    };
    assert_eq!(cursor.shape, CursorShape::Default);
}

#[test]
fn a_message_sent_the_wrong_way_is_an_error() {
    let mut host_frame = encode_host(&[HostMessage::Heartbeat]);
    assert_eq!(
        ClientMessage::decode(&mut host_frame),
        Err(DecodeError::WrongDirection { kind: 0x8d })
    );
    let mut client_frame = encode_client(&[ClientMessage::LaneFrozen]);
    assert!(matches!(
        HostMessage::decode(&mut client_frame),
        Err(DecodeError::WrongDirection { .. })
    ));
}

/// Encodes a message and sets the protocol version its preamble carries.
fn with_version(mut frame: BytesMut, version: u16) -> BytesMut {
    let at = HEADER_LEN + 4;
    frame[at..at + 2].copy_from_slice(&version.to_be_bytes());
    frame
}

#[test]
fn a_version_mismatch_is_a_typed_error_both_ways() {
    let theirs = PROTOCOL_VERSION + 1;
    let expected = DecodeError::VersionMismatch {
        ours: PROTOCOL_VERSION,
        theirs,
    };

    let hello = encode_client(&client_messages()[..1]);
    assert_eq!(
        ClientMessage::decode(&mut with_version(hello, theirs)),
        Err(expected.clone())
    );

    // A host of another version refuses with its version, which the client reads
    // as the same typed error.
    let refusal = encode_host(&[HostMessage::Refused(Refusal {
        reason: RefusalReason::VersionMismatch,
        message: String::new(),
    })]);
    assert_eq!(
        HostMessage::decode(&mut with_version(refusal, theirs)),
        Err(expected.clone())
    );
    let welcome = encode_host(&host_messages()[..1]);
    assert_eq!(
        HostMessage::decode(&mut with_version(welcome, theirs)),
        Err(expected)
    );

    let mut not_ours = encode_client(&client_messages()[..1]);
    not_ours[HEADER_LEN] = b'G';
    assert_eq!(
        ClientMessage::decode(&mut not_ours),
        Err(DecodeError::BadMagic)
    );
}

#[test]
fn encoding_refuses_what_the_peer_would_reject_and_writes_nothing() {
    let mut out = BytesMut::from(&b"kept"[..]);
    let long_title = HostMessage::WindowAdded(WindowInfo {
        title: "x".repeat(MAX_TITLE + 1),
        ..window()
    });
    assert!(matches!(
        long_title.encode(&mut out),
        Err(EncodeError::FieldTooLong { field: "title", .. })
    ));
    let huge = ClientMessage::Resize {
        window: W,
        size: Size::new(100_000, 10),
    };
    assert_eq!(
        huge.encode(&mut out),
        Err(EncodeError::InvalidValue { field: "size" })
    );
    let nan = ClientMessage::Wheel(Wheel {
        window: W,
        position: Point::default(),
        delta_x: f32::NAN,
        delta_y: 0.0,
        unit: WheelUnit::Pixels,
    });
    assert!(nan.encode(&mut out).is_err());
    assert_eq!(&out[..], b"kept");
}
