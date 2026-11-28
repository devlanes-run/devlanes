//! The hello and its refusals, what each role may send, the write cap, and
//! a real TCP listener.

use devlanes_gui_client::{ClientError, connect};
use devlanes_gui_host::Command;
use devlanes_gui_host::conformance::{self, TestPatternProbe};
use devlanes_gui_proto::{
    Buttons, HEADER_LEN, MAGIC, Point, Pointer, RefusalReason, ScreenshotResult, WindowId,
};
use tokio::net::{TcpListener, TcpStream};

use super::*;

#[tokio::test(start_paused = true)]
async fn a_hello_of_another_version_gets_a_version_refusal() {
    let host = Host::default();
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
    let host = Host::default();
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
    let tokenless = Host::start(TestPatternOptions::default(), |o| o.control_token = None);
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

#[tokio::test(start_paused = true)]
async fn a_message_the_role_may_not_send_closes_the_connection() {
    let host = Host::default();
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
    let host = Host::start(TestPatternOptions::default(), |o| o.write_queue_cap = 4096);
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

#[tokio::test]
async fn over_tcp_another_protocol_is_dropped_and_the_test_pattern_passes_conformance() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let (backend, pattern) = TestPattern::new(TestPatternOptions::default());
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
