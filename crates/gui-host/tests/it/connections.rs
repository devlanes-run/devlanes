//! The hello and its refusals, what each role may send, the write cap, and
//! a real TCP listener.

use devlanes_gui_client::{ClientError, connect};
use devlanes_gui_host::conformance::{self, TestPatternProbe};
use std::sync::atomic::AtomicBool;

use devlanes_gui_host::{
    Command, DaemonWatch, HELLO_TIMEOUT, MAX_CONNECTIONS, MAX_PENDING_HELLOS,
    MAX_SCREENSHOTS_IN_FLIGHT,
};
use devlanes_gui_proto::{
    Buttons, HEADER_LEN, MAGIC, Point, Pointer, RefusalReason, ScreenshotResult, WindowId,
};
use tokio::net::{TcpListener, TcpStream};

use super::*;

#[tokio::test(start_paused = true)]
async fn a_hello_of_another_version_gets_a_version_refusal() {
    let host = Host::default().await;
    let mut hello = BytesMut::new();
    ClientMessage::Hello(Hello {
        role: Role::Tool,
        client_id: "future".into(),
        client_name: "future".into(),
        decoders: Vec::new(),
        features: Vec::new(),
        token: None,
    })
    .encode(&mut hello)
    .expect("encodes");
    // The version follows the frame header and the magic bytes.
    let at = HEADER_LEN + MAGIC.len();
    hello[at..at + 2].copy_from_slice(&2u16.to_be_bytes());
    let mut raw = Raw {
        stream: host.pipe().await,
        buf: BytesMut::new(),
    };
    raw.stream.write_all(&hello).await.expect("writes");
    match raw.answer().await {
        HostMessage::Refused(refusal) => {
            assert_eq!(refusal.reason, RefusalReason::VersionMismatch);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(raw.closed().await.is_empty());
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_control_role_needs_the_token_and_unserved_roles_are_refused() {
    let host = Host::default().await;
    let cases = [
        (Role::Control, None, Some(RefusalReason::BadToken)),
        (Role::Control, Some("wrong"), Some(RefusalReason::BadToken)),
        (Role::Control, Some(TOKEN), None),
        (Role::Input, None, Some(RefusalReason::RoleNotSupported)),
        (
            Role::Unknown(9),
            None,
            Some(RefusalReason::RoleNotSupported),
        ),
    ];
    for (role, token, refused) in cases {
        let mut raw = Raw::hello(&host, role, token).await;
        match (raw.answer().await, refused) {
            (HostMessage::Refused(refusal), Some(reason)) => {
                assert_eq!(refusal.reason, reason, "{role:?} {token:?}");
            }
            (HostMessage::Welcome(_), None) => {}
            (answer, _) => panic!("{role:?} {token:?}: {answer:?}"),
        }
    }
    host.stop().await;

    // A host the daemon gave no token serves no control connection.
    let tokenless = Host::start(TestPatternOptions::default(), |o| o.control_token = None).await;
    let options =
        ClientOptions::new(Role::Control, "daemon", "devlanes").with_token(Token::new(TOKEN));
    let refused = connect_stream(tokenless.pipe().await, options).await;
    assert!(
        matches!(
            refused,
            Err(ClientError::Refused {
                reason: RefusalReason::RoleNotSupported,
                ..
            })
        ),
        "{refused:?}"
    );
    tokenless.stop().await;
}

/// A host the daemon started, whose unit counts as running while `alive` is set.
async fn watched(timeout: Duration, alive: Arc<AtomicBool>) -> Host {
    Host::start(TestPatternOptions::default(), |o| {
        o.daemon = Some(DaemonWatch {
            identity_timeout: timeout,
            unit_alive: Arc::new(move |_, _| alive.load(Ordering::SeqCst)),
        })
    })
    .await
}

/// How `served` ended, within [`PATIENCE`].
async fn ended(served: JoinHandle<Result<(), ServeError>>) -> Result<(), ServeError> {
    tokio::time::timeout(PATIENCE, served)
        .await
        .expect("the host stops")
        .expect("the host task did not panic")
}

#[tokio::test(start_paused = true)]
async fn a_host_stops_by_itself_once_its_daemon_is_gone() {
    let timeout = Duration::from_secs(30);
    let alive = Arc::new(AtomicBool::new(true));

    // No control connection at all: the deadline.
    let host = watched(timeout, alive.clone()).await;
    let started = tokio::time::Instant::now();
    let result = ended(host.served).await;
    assert!(
        matches!(result, Err(ServeError::NoUnitIdentity)),
        "{result:?}"
    );
    assert!(started.elapsed() >= timeout);

    // A control connection that closes before the identity: at once.
    let host = watched(timeout, alive.clone()).await;
    let (daemon, _events) = host.control().await;
    let started = tokio::time::Instant::now();
    daemon.close();
    let result = ended(host.served).await;
    assert!(
        matches!(result, Err(ServeError::NoUnitIdentity)),
        "{result:?}"
    );
    assert!(started.elapsed() < timeout);

    // After the identity the host outlives the deadline and its control
    // connection while the unit runs, and ends once the unit is gone.
    let host = watched(timeout, alive.clone()).await;
    let (daemon, _events) = host.control().await;
    daemon.unit_process(4242, 7, None).expect("sends");
    eventually("the identity reaches the backend", || {
        host.pattern
            .commands()
            .iter()
            .any(|c| matches!(c, Command::UnitIdentity { pid: 4242, .. }))
    })
    .await;
    daemon.close();
    tokio::time::sleep(timeout * 2).await;
    assert!(!host.served.is_finished());
    alive.store(false, Ordering::SeqCst);
    assert!(ended(host.served).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn the_units_exit_closes_windows_and_a_plain_stop_does_not() {
    // A stop without the unit's exit, as for a restart: the viewer's window
    // is not Closed when the connection goes.
    let host = Host::default().await;
    let (_viewer, mut events) = host.viewer("ada").await;
    host.shutdown.cancel();
    let mut closed = false;
    while let Some(event) = events.next().await {
        match event {
            Event::Lock {
                state: devlanes_gui_proto::LockState::Closed,
                ..
            } => closed = true,
            Event::Closed(_) => break,
            _ => {}
        }
    }
    assert!(!closed, "a stop without the unit's exit closed the window");
    assert!(ended(host.served).await.is_ok());

    // The unit's exit: Closed, and the host ends by itself.
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (_viewer, mut events) = host.viewer("ada").await;
    let (daemon, _control) = host.control().await;
    daemon.unit_exited(Some(0)).expect("sends");
    until(&mut events, |e| {
        matches!(e, Event::Lock { window: w, state: devlanes_gui_proto::LockState::Closed } if *w == window)
            .then_some(())
    })
    .await;
    assert!(ended(host.served).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_message_the_role_may_not_send_closes_the_connection() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let press = ClientMessage::Pointer(Pointer {
        window,
        position: Point::new(5, 5),
        buttons: Buttons::LEFT,
        screen_delta: None,
    });
    let second_hello = ClientMessage::Hello(Hello {
        role: Role::Control,
        client_id: "again".into(),
        client_name: "again".into(),
        decoders: Vec::new(),
        features: Vec::new(),
        token: Some(Token::new(TOKEN)),
    });
    let cases = [
        (Role::Tool, ClientMessage::FrameAck { window, seq: 1 }),
        (Role::Control, press),
        (Role::Viewer, ClientMessage::UnitExited { code: None }),
        (Role::Tool, second_hello),
    ];
    for (role, message) in cases {
        let mut raw = Raw::welcomed(&host, role).await;
        raw.send(&message).await;
        raw.closed().await;
    }
    let reached = |c: &Command| matches!(c, Command::Pointer(_) | Command::UnitExited { .. });
    assert!(!host.pattern.commands().iter().any(reached));

    // The same message from the role that may send it is served.
    let mut daemon = Raw::welcomed(&host, Role::Control).await;
    daemon
        .send(&ClientMessage::UnitExited { code: Some(0) })
        .await;
    eventually("the exit reaches the backend", || {
        host.pattern.commands().iter().any(reached)
    })
    .await;
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_client_that_does_not_read_is_closed_past_the_write_cap() {
    let host = Host::start(TestPatternOptions::default(), |o| o.write_queue_cap = 4096).await;
    // A small pipe fills at once, so the answers pile up in the host's queue.
    let mut flood = Raw {
        stream: host.pipe_with(256).await,
        buf: BytesMut::new(),
    };
    flood
        .send(&ClientMessage::Hello(Hello {
            role: Role::Tool,
            client_id: "flood".into(),
            client_name: "flood".into(),
            decoders: Vec::new(),
            features: Vec::new(),
            token: None,
        }))
        .await;
    let requests = 5000;
    let mut out = BytesMut::new();
    for request in 0..requests {
        // A window that does not exist is answered at once, without a capture.
        ClientMessage::Screenshot {
            request,
            window: Some(WindowId(999)),
        }
        .encode(&mut out)
        .expect("encodes");
    }
    // Fails once the host closes the connection, which is the point.
    let _ = flood.stream.write_all(&out).await;
    // Without reading a byte: the host drops the stream although its writer
    // is stuck on the full pipe, so writing here fails.
    let mut keep_alive = BytesMut::new();
    ClientMessage::KeepAlive
        .encode(&mut keep_alive)
        .expect("encodes");
    let dropped = async {
        while flood.stream.write_all(&keep_alive).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(PATIENCE, dropped)
        .await
        .expect("the host dropped the connection");
    let answered = flood
        .closed()
        .await
        .into_iter()
        .filter(|m| {
            matches!(
                m,
                HostMessage::Screenshot {
                    result: ScreenshotResult::NoSuchWindow,
                    ..
                }
            )
        })
        .count();
    assert!(answered < requests as usize, "{answered} answers");

    // The host itself goes on.
    let (tool, _events) = host.tool("ada").await;
    tool.screenshot(None).await.expect("a screenshot");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_connection_without_a_hello_is_closed_after_the_hello_timeout() {
    let host = Host::default().await;
    let mut silent = Raw {
        stream: host.pipe().await,
        buf: BytesMut::new(),
    };
    let opened = tokio::time::Instant::now();
    assert!(silent.closed().await.is_empty());
    assert!(opened.elapsed() >= HELLO_TIMEOUT);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn connections_past_the_limit_are_refused() {
    let host = Host::default().await;
    let mut open = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        open.push(Raw::welcomed(&host, Role::Tool).await);
    }
    let mut extra = Raw::hello(&host, Role::Tool, None).await;
    match extra.answer().await {
        HostMessage::Refused(refusal) => assert_eq!(refusal.reason, RefusalReason::Other),
        other => panic!("expected a refusal, got {other:?}"),
    }

    // A place frees up when a connection closes.
    drop(open.pop());
    let welcomed = async {
        loop {
            let mut next = Raw::hello(&host, Role::Tool, None).await;
            if let Some(HostMessage::Welcome(_)) = next.read().await {
                return next;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(PATIENCE, welcomed)
        .await
        .expect("welcomed once a place is free");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn silent_connections_cannot_keep_new_ones_out() {
    let host = Host::default().await;
    let (welcomed, _events) = host.tool("welcomed").await;
    let mut silent = Vec::new();
    for _ in 0..MAX_PENDING_HELLOS {
        silent.push(Raw {
            stream: host.pipe().await,
            buf: BytesMut::new(),
        });
    }
    // One more arrives, and the silent one that waited longest makes room.
    let opened = tokio::time::Instant::now();
    let daemon = Raw::welcomed(&host, Role::Control).await;
    assert!(silent[0].closed().await.is_empty());
    assert!(
        opened.elapsed() < HELLO_TIMEOUT,
        "without waiting for the timeout"
    );
    drop(daemon);
    // The connection welcomed before them goes on.
    welcomed.screenshot(None).await.expect("a screenshot");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_fifth_screenshot_in_progress_closes_the_connection() {
    let host = Host::default().await;
    // Nobody watches the window, so each screenshot waits for a frame the
    // pattern holds back.
    host.pattern.hold_frames(true);
    let mut tool = Raw::welcomed(&host, Role::Tool).await;
    let window = Some(host.pattern.first_window());
    for request in 0..MAX_SCREENSHOTS_IN_FLIGHT as u32 {
        tool.send(&ClientMessage::Screenshot { request, window })
            .await;
    }
    // Four are allowed: they wait, and the connection stays open.
    tool.round_trip().await;
    tool.send(&ClientMessage::Screenshot {
        request: 99,
        window,
    })
    .await;
    let after = tool.closed().await;
    let answered = after
        .iter()
        .filter(|m| matches!(m, HostMessage::Screenshot { .. }))
        .count();
    assert_eq!(answered, 0, "{after:?}");
    host.stop().await;
}

#[tokio::test]
async fn over_tcp_another_protocol_is_dropped_and_the_test_pattern_passes_conformance() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let (backend, pattern) = TestPattern::new(TestPatternOptions::default());
    let backend = start_backend(backend).await.expect("starts");
    let options = ServeOptions::new("acme:desktop");
    let shutdown = options.shutdown.clone();
    let served = tokio::spawn(serve(listener, backend, options));

    let mut http = TcpStream::connect(addr).await.expect("connects");
    http.write_all(b"GET / HTTP/1.1\r\nHost: acme.localhost\r\n\r\n")
        .await
        .expect("writes");
    let mut got = Vec::new();
    // A reset instead of a clean close is fine too; the host answers nothing.
    let _ = tokio::time::timeout(PATIENCE, http.read_to_end(&mut got))
        .await
        .expect("the host closes the connection");
    assert!(got.is_empty(), "{got:?}");

    let (client, mut events) = connect(addr, ClientOptions::new(Role::Tool, "ci", "conformance"))
        .await
        .expect("connects");
    let mut probe = TestPatternProbe::new(pattern.clone(), pattern.first_window());
    conformance::run(&client, &mut events, &mut probe)
        .await
        .expect("the test pattern passes");
    client.close();
    shutdown.cancel();
    let result = tokio::time::timeout(PATIENCE, served)
        .await
        .expect("stops")
        .expect("no panic");
    assert!(result.is_ok(), "{result:?}");
}
