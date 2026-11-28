//! What the TUI's daemon client sends, read off a fake daemon's socket: the
//! params of the requests that pair by ticket, which the in-memory
//! `FakeApi` of the other tests never sees.

use std::time::Duration;

use devlanes_client::{Frame, Hello, Method, Paths};
use devlanes_ipc::{Connection, local::LocalListener};
use devlanes_proto::{Control, Outcome, Response};
use devlanes_tui::{ClientApi, ConnectEvent, DaemonApi, connect::TicketText};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::timeout};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Daemon paths in `home`, with the socket inside the test's directory
/// even when the host exports `XDG_RUNTIME_DIR`. A Windows pipe is named
/// after the home, so it is unique.
fn test_paths(home: &std::path::Path) -> devlanes_client::Result<Paths> {
    #[cfg_attr(windows, expect(unused_mut, reason = "the pipe name stays"))]
    let mut paths = Paths::new(home)?;
    #[cfg(unix)]
    {
        paths.socket = paths.run.join("daemon.sock");
    }
    Ok(paths)
}

/// Answers every request on every connection with `{}`, sending each
/// request's method and params to `seen`.
async fn record(
    mut listener: LocalListener,
    seen: mpsc::UnboundedSender<(Method, Value)>,
) -> Result<(), devlanes_ipc::Error> {
    loop {
        let stream = listener.accept().await?;
        let seen = seen.clone();
        tokio::spawn(async move {
            let mut connection = Connection::new(stream);
            connection.recv().await?;
            connection
                .send(Frame::Control(Control::Hello(Hello::current(0))))
                .await?;
            while let Ok(Frame::Control(Control::Request(request))) = connection.recv().await {
                let _ = seen.send((request.method, request.params));
                connection
                    .send(Frame::Control(Control::Response(Response {
                        id: request.id,
                        result: Outcome::Ok { data: json!({}) },
                    })))
                    .await?;
            }
            Ok::<_, devlanes_ipc::Error>(())
        });
    }
}

/// `p` asks for a ticket and a code, `t` for the ticket alone, the form
/// pairs with `{ticket, name}` and forgetting a box sends `forget`: the
/// params the daemon reads, as `Method::RemotePair`, `Method::Connect` and
/// `Method::Disconnect` document them.
#[tokio::test]
async fn pairing_requests_carry_the_params_the_daemon_reads() -> TestResult {
    timeout(Duration::from_secs(20), async {
        let temp = tempfile::tempdir()?;
        let paths = test_paths(&temp.path().join("home"))?;
        paths.prepare()?;
        let listener = LocalListener::bind(&paths.socket).await?;
        let (seen_tx, mut seen) = mpsc::unbounded_channel();
        let server = tokio::spawn(record(listener, seen_tx));
        let api = ClientApi::connect(paths, None, None).await?;
        api.pair().await?;
        api.mint_ticket().await?;
        let mut session = api
            .pair_ticket(TicketText::new("dlp1exampleticket"), Some("acme".into()))
            .await?;
        while let Some(event) = session.events.recv().await {
            if let ConnectEvent::Done(result) = event {
                result?;
                break;
            }
        }
        api.forget("acme".into()).await?;
        let mut requests = Vec::new();
        for _ in 0..4 {
            requests.push(seen.recv().await.ok_or("the fake daemon stopped")?);
        }
        server.abort();
        assert_eq!(
            requests,
            [
                (Method::RemotePair, json!({"ticket": true})),
                (Method::RemotePair, json!({"ticket": true, "code": false})),
                (
                    Method::Connect,
                    json!({"ticket": "dlp1exampleticket", "name": "acme"})
                ),
                (Method::Disconnect, json!({"host": "acme", "forget": true})),
            ]
        );
        Ok(())
    })
    .await?
}
