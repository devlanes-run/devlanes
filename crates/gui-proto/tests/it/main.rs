//! The wire format through the public API: round trips, chunked input,
//! malformed and oversized frames, compatibility rules, roles.

use bytes::{BufMut, Bytes, BytesMut};
use devlanes_gui_proto::{
    Buttons, ClientMessage, CodecId, CodecOffer, Cursor, CursorImage, CursorShape, DecodeError,
    Edge, EncodeError, HEADER_LEN, Hello, Holder, HostMessage, Key, KeyCode, LockState, MAGIC,
    MAX_APP_ID, MAX_CLIPBOARD, MAX_CODECS, MAX_CURSOR_SIDE, MAX_FEATURE, MAX_FEATURES,
    MAX_FRAME_PAYLOAD, MAX_ID, MAX_MESSAGE, MAX_NAME, MAX_TEXT, MAX_TITLE, MAX_UNKNOWN_BODY,
    MAX_WINDOWS, MoveResize, PROTOCOL_VERSION, Point, Pointer, Refusal, RefusalReason, Role,
    Screenshot, ScreenshotResult, SessionId, Size, Token, VideoFrame, Welcome, Wheel, WheelUnit,
    WindowId, WindowInfo, WindowKind,
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
        kind: WindowKind::Popup,
    }
}

/// A hello with `role`.
fn hello(role: Role) -> Hello {
    Hello {
        role,
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
        token: Some(Token::new("0123abcd")),
    }
}

/// One of every client message, with values that use each field's range.
fn client_messages() -> Vec<ClientMessage> {
    let all = vec![
        ClientMessage::Hello(hello(Role::Control)),
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
        ClientMessage::PointerLeft { window: W },
        ClientMessage::Wheel(Wheel {
            window: W,
            position: Point::new(10, 20),
            delta_x: -0.25,
            delta_y: 120.5,
            unit: WheelUnit::Lines,
        }),
        ClientMessage::Key(Key {
            window: W,
            code: KeyCode::new(KeyCode::CONSUMER_PAGE, 0xe9),
            down: true,
        }),
        ClientMessage::Text {
            window: W,
            text: "héllo 🦀".into(),
        },
        ClientMessage::Resize {
            window: W,
            size: Size::new(1200, 700),
            request: 3,
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
        ClientMessage::KeepAlive,
        ClientMessage::Sync { request: 4 },
        ClientMessage::UnitProcess {
            pid: 31337,
            start_time: 1_234_567_890,
            job: Some("devlanes-acme-web".into()),
        },
        ClientMessage::UnitExited { code: Some(-1) },
        ClientMessage::LaneFrozen,
        ClientMessage::LaneThawed,
    ];
    // Fails to compile when a variant is added, so this list grows with it.
    let mut seen = [false; 19];
    for message in &all {
        let index = match message {
            ClientMessage::Hello(_) => 0,
            ClientMessage::FrameAck { .. } => 1,
            ClientMessage::KeyframeRequest { .. } => 2,
            ClientMessage::Pointer(_) => 3,
            ClientMessage::PointerLeft { .. } => 4,
            ClientMessage::Wheel(_) => 5,
            ClientMessage::Key(_) => 6,
            ClientMessage::Text { .. } => 7,
            ClientMessage::Resize { .. } => 8,
            ClientMessage::Close { .. } => 9,
            ClientMessage::Focus { .. } => 10,
            ClientMessage::Clipboard { .. } => 11,
            ClientMessage::Screenshot { .. } => 12,
            ClientMessage::KeepAlive => 13,
            ClientMessage::Sync { .. } => 14,
            ClientMessage::UnitProcess { .. } => 15,
            ClientMessage::UnitExited { .. } => 16,
            ClientMessage::LaneFrozen => 17,
            ClientMessage::LaneThawed => 18,
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
            session: SessionId(u64::MAX),
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
                    kind: WindowKind::Tooltip,
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
            kind: WindowKind::Dialog,
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
            session: SessionId(2),
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
        HostMessage::ResizeApplied {
            window: W,
            request: 3,
            from_seq: 12,
        },
        HostMessage::Synced { request: 4 },
        HostMessage::Activity,
    ];
    // Fails to compile when a variant is added, so this list grows with it.
    let mut seen = [false; 16];
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
            HostMessage::ResizeApplied { .. } => 13,
            HostMessage::Synced { .. } => 14,
            HostMessage::Activity => 15,
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
    all.push(HostMessage::LockChanged {
        window: W,
        state: LockState::Locked,
    });
    all
}

/// Every hello role, the reserved one and an unknown one included.
fn hellos() -> Vec<ClientMessage> {
    [
        Role::Viewer,
        Role::Tool,
        Role::Control,
        Role::Input,
        Role::Unknown(200),
    ]
    .into_iter()
    .map(|role| {
        ClientMessage::Hello(Hello {
            token: None,
            ..hello(role)
        })
    })
    .collect()
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
    let mut client = client_messages();
    client.extend(hellos());
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

#[test]
fn the_largest_valid_messages_fit_their_limits() {
    let full = |n: usize| "é".repeat(n / 2);
    let full_window = |id| WindowInfo {
        id: WindowId(id),
        title: full(MAX_TITLE),
        app_id: full(MAX_APP_ID),
        max_size: Some(Size::new(8192, 8192)),
        ..window()
    };
    let offers = vec![
        CodecOffer {
            codec: CodecId::H264,
            hardware: true
        };
        MAX_CODECS
    ];
    let features = vec!["f".repeat(MAX_FEATURE); MAX_FEATURES];
    let host = vec![
        HostMessage::Welcome(Welcome {
            session: SessionId(1),
            service: full(MAX_NAME),
            encoders: offers.clone(),
            real_desktop: false,
            windows: (0..MAX_WINDOWS as u32).map(full_window).collect(),
            features: features.clone(),
        }),
        HostMessage::WindowChanged(full_window(1)),
        HostMessage::Cursor(Cursor {
            window: W,
            shape: CursorShape::Image(CursorImage {
                size: Size::new(MAX_CURSOR_SIDE, MAX_CURSOR_SIDE),
                hotspot: Point::new(0, 0),
                pixels: Bytes::from(vec![7; (MAX_CURSOR_SIDE * MAX_CURSOR_SIDE * 4) as usize]),
            }),
            position: Some(Point::new(1, 1)),
        }),
        HostMessage::InputHolder(Some(Holder {
            session: SessionId(1),
            client_id: "i".repeat(MAX_ID),
            name: "n".repeat(MAX_NAME),
        })),
        HostMessage::Refused(Refusal {
            reason: RefusalReason::Other,
            message: "m".repeat(MAX_MESSAGE),
        }),
        HostMessage::Clipboard {
            text: "c".repeat(MAX_CLIPBOARD),
        },
        HostMessage::Screenshot {
            request: 1,
            result: ScreenshotResult::Failed("m".repeat(MAX_MESSAGE)),
        },
    ];
    let client = vec![
        ClientMessage::Hello(Hello {
            role: Role::Viewer,
            client_id: "i".repeat(MAX_ID),
            client_name: "n".repeat(MAX_NAME),
            decoders: offers,
            features,
            token: Some(Token::new("t".repeat(MAX_ID))),
        }),
        ClientMessage::Text {
            window: W,
            text: "t".repeat(MAX_TEXT),
        },
        ClientMessage::Clipboard {
            text: "c".repeat(MAX_CLIPBOARD),
        },
        ClientMessage::UnitProcess {
            pid: u32::MAX,
            start_time: u64::MAX,
            job: Some("j".repeat(MAX_MESSAGE)),
        },
    ];
    assert_eq!(
        decode_in_chunks(&encode_host(&host), [], HostMessage::decode),
        host
    );
    assert_eq!(
        decode_in_chunks(&encode_client(&client), [], ClientMessage::decode),
        client
    );
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
    let mut client = client_messages();
    client.extend(hellos());
    for message in client {
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
fn a_malformed_value_in_a_complete_body_names_its_field() {
    /// Encodes `message` and writes `bytes` at `offset` into its body.
    fn patched(message: Message, offset: usize, bytes: &[u8]) -> Result<(), DecodeError> {
        let mut frame = match &message {
            Message::Client(m) => encode_client(std::slice::from_ref(m)),
            Message::Host(m) => encode_host(std::slice::from_ref(m)),
        };
        let at = HEADER_LEN + offset;
        frame[at..at + bytes.len()].copy_from_slice(bytes);
        match message {
            Message::Client(_) => ClientMessage::decode(&mut frame).map(|_| ()),
            Message::Host(_) => HostMessage::decode(&mut frame).map(|_| ()),
        }
    }
    enum Message {
        Client(ClientMessage),
        Host(HostMessage),
    }
    let invalid = |kind, field, value| DecodeError::InvalidValue { kind, field, value };
    let pointer = ClientMessage::Pointer(Pointer {
        window: W,
        position: Point::new(1, 1),
        buttons: Buttons::NONE,
        screen_delta: None,
    });
    let wheel = || {
        ClientMessage::Wheel(Wheel {
            window: W,
            position: Point::new(1, 1),
            delta_x: 1.0,
            delta_y: 1.0,
            unit: WheelUnit::Pixels,
        })
    };
    let nan = f32::NAN.to_bits();
    let host = host_messages();
    let cursor = host[6].clone();
    let cases: Vec<(Message, usize, Vec<u8>, DecodeError)> = vec![
        (
            Message::Client(ClientMessage::Focus {
                window: W,
                focused: true,
            }),
            4,
            vec![2],
            invalid(0x0a, "focused", 2),
        ),
        (
            Message::Client(pointer),
            13,
            vec![3],
            invalid(0x04, "screen delta", 3),
        ),
        (
            Message::Client(wheel()),
            12,
            nan.to_be_bytes().to_vec(),
            invalid(0x05, "wheel delta", nan.into()),
        ),
        (
            Message::Client(wheel()),
            20,
            vec![2],
            invalid(0x05, "wheel unit", 2),
        ),
        (
            Message::Client(ClientMessage::Resize {
                window: W,
                size: Size::new(10, 10),
                request: 1,
            }),
            4,
            9000u32.to_be_bytes().to_vec(),
            invalid(0x08, "size", 9000),
        ),
        (
            Message::Client(ClientMessage::Text {
                window: W,
                text: "ok".into(),
            }),
            8,
            vec![0xff],
            DecodeError::InvalidUtf8 {
                kind: 0x07,
                field: "text",
            },
        ),
        (
            Message::Host(host[7].clone()),
            4,
            vec![9],
            invalid(0x88, "edge", 9),
        ),
        (
            Message::Host(host[5].clone()),
            14,
            vec![0; 4],
            invalid(0x86, "frame size", 0),
        ),
        (
            Message::Host(host[11].clone()),
            9,
            vec![0; 4],
            invalid(0x8c, "screenshot size", 0),
        ),
        // The image says 2x1 (8 bytes) and its pixel string holds 4.
        (
            Message::Host(cursor.clone()),
            21,
            4u32.to_be_bytes().to_vec(),
            invalid(0x87, "cursor pixels", 4),
        ),
        (
            Message::Host(cursor),
            5,
            vec![0; 4],
            invalid(0x87, "cursor size", 1),
        ),
        (
            Message::Host(host[3].clone()),
            0,
            vec![0xff, 0xff],
            DecodeError::Truncated {
                kind: 0x84,
                field: "window",
            },
        ),
    ];
    for (message, offset, bytes, expected) in cases {
        assert_eq!(patched(message, offset, &bytes), Err(expected));
    }
}

#[test]
fn random_bodies_and_mutated_frames_never_panic() {
    // A fixed xorshift, so a failure reproduces.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let decode = |frame: &mut BytesMut| {
        let result = match frame[0] < 0x80 {
            true => ClientMessage::decode(frame).map(|_| ()),
            false => HostMessage::decode(frame).map(|_| ()),
        };
        // Errors are fine; only reaching here without a panic matters.
        drop(result);
    };
    for _ in 0..20_000 {
        let kind = (next() % 0xc0) as u8;
        let len = (next() % 96) as usize;
        let mut frame = BytesMut::new();
        frame.put_u8(kind);
        frame.put_u32(len as u32);
        frame.extend((0..len).map(|_| next() as u8));
        decode(&mut frame);
    }

    // Valid frames with one byte changed reach the checks deep in each body.
    let mut frames: Vec<BytesMut> = client_messages()
        .iter()
        .chain(&hellos())
        .map(|m| encode_client(std::slice::from_ref(m)))
        .collect();
    frames.extend(
        host_messages()
            .iter()
            .chain(&more_host_messages())
            .map(|m| encode_host(std::slice::from_ref(m))),
    );
    for frame in frames {
        for at in HEADER_LEN..frame.len() {
            for value in [0x00, 0xff, frame[at] ^ 0x01, frame[at] ^ 0x80] {
                let mut mutated = frame.clone();
                mutated[at] = value;
                decode(&mut mutated);
            }
        }
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
        session: SessionId(1),
        service: String::new(),
        encoders: Vec::new(),
        real_desktop: false,
        windows: Vec::new(),
        features: Vec::new(),
    })]);
    // magic (4), version (2), session (8), service length (4), then the encoder count.
    let count = HEADER_LEN + 18;
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

    // Unknown cursor names fall back to the arrow.
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
fn a_later_welcome_with_longer_windows_and_offers_still_decodes() {
    // A window as this version writes it: a u16 length, then its fields.
    let added = encode_host(&[HostMessage::WindowAdded(window())]);
    let mut longer_window = BytesMut::new();
    longer_window.put_u16(u16::from_be_bytes([added[HEADER_LEN], added[HEADER_LEN + 1]]) + 2);
    longer_window.extend_from_slice(&added[HEADER_LEN + 2..]);
    longer_window.extend_from_slice(&[0xaa, 0xbb]);
    // An unknown window kind falls back to a top-level window.
    let kind_at = longer_window.len() - 3;
    longer_window[kind_at] = 0x40;

    let mut body = BytesMut::new();
    body.extend_from_slice(&MAGIC);
    body.put_u16(PROTOCOL_VERSION);
    body.put_u64(5);
    body.put_u32(4);
    body.extend_from_slice(b"acme");
    body.put_u16(1);
    body.extend_from_slice(&[0, 4, CodecId::H264.0, 1, 0xcc, 0xdd]);
    body.put_u8(1);
    body.put_u16(1);
    body.extend_from_slice(&longer_window);
    body.put_u16(0);
    body.extend_from_slice(&[0xee]);
    let mut frame = BytesMut::new();
    frame.put_u8(0x81);
    frame.put_u32(body.len() as u32);
    frame.extend_from_slice(&body);

    let expected = HostMessage::Welcome(Welcome {
        session: SessionId(5),
        service: "acme".into(),
        encoders: vec![CodecOffer {
            codec: CodecId::H264,
            hardware: true,
        }],
        real_desktop: true,
        windows: vec![WindowInfo {
            kind: WindowKind::Toplevel,
            ..window()
        }],
        features: Vec::new(),
    });
    assert_eq!(HostMessage::decode(&mut frame), Ok(Some(expected)));
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

#[test]
fn a_first_frame_that_is_not_the_handshake_is_bad_magic() {
    let mut http = BytesMut::from(&b"G"[..]);
    assert_eq!(
        ClientMessage::decode_hello(&mut http),
        Err(DecodeError::BadMagic)
    );
    let mut early = encode_client(&[ClientMessage::KeepAlive]);
    assert_eq!(
        ClientMessage::decode_hello(&mut early),
        Err(DecodeError::BadMagic)
    );
    let mut partial = encode_client(&hellos()[..1]);
    let whole = partial.split_off(0);
    partial.extend_from_slice(&whole[..3]);
    assert_eq!(ClientMessage::decode_hello(&mut partial), Ok(None));
    partial.extend_from_slice(&whole[3..]);
    assert!(matches!(
        ClientMessage::decode_hello(&mut partial),
        Ok(Some(Hello {
            role: Role::Viewer,
            ..
        }))
    ));

    let mut heartbeat = encode_host(&[HostMessage::Heartbeat]);
    assert_eq!(
        HostMessage::decode_answer(&mut heartbeat),
        Err(DecodeError::BadMagic)
    );
    let mut refusal = encode_host(&host_messages()[1..2]);
    assert!(matches!(
        HostMessage::decode_answer(&mut refusal),
        Ok(Some(HostMessage::Refused(_)))
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
        ClientMessage::decode_hello(&mut with_version(hello, theirs)),
        Err(expected.clone())
    );

    // A host of another version refuses with its version, which the client reads
    // as the same typed error.
    let refusal = encode_host(&[HostMessage::Refused(Refusal {
        reason: RefusalReason::VersionMismatch,
        message: String::new(),
    })]);
    assert_eq!(
        HostMessage::decode_answer(&mut with_version(refusal, theirs)),
        Err(expected.clone())
    );
    let welcome = encode_host(&host_messages()[..1]);
    assert_eq!(
        HostMessage::decode_answer(&mut with_version(welcome, theirs)),
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
fn each_role_may_send_only_its_own_messages() {
    let roles = [
        Role::Viewer,
        Role::Tool,
        Role::Control,
        Role::Input,
        Role::Unknown(9),
    ];
    for message in client_messages() {
        let allowed: Vec<Role> = roles
            .into_iter()
            .filter(|role| message.allowed_for(*role))
            .collect();
        let expected: &[Role] = match &message {
            ClientMessage::Hello(_) => &[],
            ClientMessage::UnitProcess { .. }
            | ClientMessage::UnitExited { .. }
            | ClientMessage::LaneFrozen
            | ClientMessage::LaneThawed => &[Role::Control],
            ClientMessage::FrameAck { .. } | ClientMessage::KeyframeRequest { .. } => {
                &[Role::Viewer, Role::Input]
            }
            ClientMessage::Screenshot { .. } => &[Role::Viewer, Role::Tool],
            _ => &[Role::Viewer, Role::Tool, Role::Input],
        };
        assert_eq!(allowed, expected, "{message:?}");
    }
}

#[test]
fn a_token_never_shows_in_debug_output() {
    let message = ClientMessage::Hello(hello(Role::Control));
    let printed = format!("{message:?}");
    assert!(printed.contains("Token(<redacted>)"));
    assert!(!printed.contains("0123abcd"));
    assert_eq!(Token::new("0123abcd"), Token::new("0123abcd"));
    assert_ne!(Token::new("0123abcd"), Token::new("0123abce"));
    assert_ne!(Token::new("0123abcd"), Token::new("0123abc"));
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
        request: 1,
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
