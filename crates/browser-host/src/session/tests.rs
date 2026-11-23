//! The session against a scripted daemon, through in-memory pipes.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use devlanes_client::{Context, Method, RpcError};
use devlanes_proto::Event as DaemonEvent;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncWriteExt, DuplexStream, duplex},
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout},
};

use super::*;
use crate::daemon::{Events, Queries};

/// How the scripted daemon answers a request: the method, its params and
/// how many requests of that method came before it.
type Answer = dyn Fn(Method, &Value, usize) -> Result<Value, CallError> + Send + Sync;

/// What the scripted daemon saw and how it answers.
struct Script {
    answer: Box<Answer>,
    /// Every request, with the lane of its context.
    calls: std::sync::Mutex<Vec<(Method, Value, Option<String>)>>,
    /// The next subscription's events. `None` reads as a stopped daemon.
    events: std::sync::Mutex<Option<mpsc::Receiver<DaemonEvent>>>,
}

/// A daemon that answers from a [`Script`].
#[derive(Clone)]
struct Scripted(Arc<Script>);

impl Scripted {
    /// A daemon that answers with `answer` and has no subscription yet.
    fn new(
        answer: impl Fn(Method, &Value, usize) -> Result<Value, CallError> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Script {
            answer: Box::new(answer),
            calls: std::sync::Mutex::new(Vec::new()),
            events: std::sync::Mutex::new(None),
        }))
    }

    /// Makes the next subscription succeed, with events from the returned sender.
    fn subscribable(&self) -> mpsc::Sender<DaemonEvent> {
        let (sender, receiver) = mpsc::channel(16);
        *self.0.events.lock().unwrap_or_else(|e| e.into_inner()) = Some(receiver);
        sender
    }

    /// The requests of `method` so far, with their params and lane.
    fn calls(&self, method: Method) -> Vec<(Value, Option<String>)> {
        self.0
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(called, _, _)| *called == method)
            .map(|(_, params, lane)| (params.clone(), lane.clone()))
            .collect()
    }
}

#[async_trait]
impl Daemon for Scripted {
    /// Always connects.
    async fn queries(&self) -> Result<Box<dyn Queries>, Unavailable> {
        Ok(Box::new(self.clone()))
    }

    /// Subscribes once per [`Scripted::subscribable`], and is stopped otherwise.
    async fn events(&self) -> Result<Box<dyn Events>, Unavailable> {
        let receiver = self
            .0
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match receiver {
            Some(receiver) => Ok(Box::new(Subscription(receiver))),
            None => Err(Unavailable::Stopped(daemon::STOPPED.into())),
        }
    }
}

#[async_trait]
impl Queries for Scripted {
    /// Records the request and answers from the script.
    async fn call(
        &mut self,
        method: Method,
        params: Value,
        context: Context,
    ) -> Result<Value, CallError> {
        let before = self.calls(method).len();
        self.0
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((method, params.clone(), context.lane));
        (self.0.answer)(method, &params, before)
    }
}

/// The scripted events of one subscription.
struct Subscription(mpsc::Receiver<DaemonEvent>);

#[async_trait]
impl Events for Subscription {
    /// The next scripted event; dropping the sender ends the subscription.
    async fn next(&mut self) -> Option<DaemonEvent> {
        self.0.recv().await
    }
}

/// The extension's end of a session.
struct Port {
    to_host: DuplexStream,
    from_host: DuplexStream,
    session: JoinHandle<Result<(), FrameError>>,
}

impl Port {
    /// Starts a session with `daemon`.
    fn open(daemon: Scripted) -> Self {
        let (to_host, host_in) = duplex(1 << 16);
        let (host_out, from_host) = duplex(1 << 21);
        Self {
            to_host,
            from_host,
            session: tokio::spawn(serve(daemon, host_in, host_out)),
        }
    }

    /// Sends one framed message.
    async fn send(&mut self, message: Value) {
        let body = message.to_string();
        let mut bytes = (body.len() as u32).to_ne_bytes().to_vec();
        bytes.extend_from_slice(body.as_bytes());
        self.to_host
            .write_all(&bytes)
            .await
            .expect("the host reads");
    }

    /// The next message from the host, failing after a minute of quiet.
    async fn recv(&mut self) -> Value {
        let body = timeout(Duration::from_secs(60), frame::read(&mut self.from_host))
            .await
            .expect("the host answers in time")
            .expect("the frame reads")
            .expect("the host has not closed");
        serde_json::from_slice(&body).expect("the host sends JSON")
    }

    /// Whether the host stays quiet for `wait`.
    async fn quiet(&mut self, wait: Duration) -> bool {
        timeout(wait, frame::read(&mut self.from_host))
            .await
            .is_err()
    }

    /// Sends `hello` and checks its reply.
    async fn hello(&mut self) {
        self.send(
            json!({"id": 1, "method": "hello", "params": {"protocol": 1, "extension": "0.1.0"}}),
        )
        .await;
        let reply = self.recv().await;
        assert_eq!(reply["result"]["daemon"], "running", "{reply}");
    }
}

/// A `local_lane_running` refusal for local lane `acme@0`, with `extra` data.
fn refused(extra: Value) -> CallError {
    let mut data = json!({
        "local_lane": "acme@0",
        "holds": "processes",
        "message": "acme@0 runs on this machine. Stop it with devlanes stop --lane acme@0.",
    });
    if let (Some(data), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
        data.extend(extra.clone());
    }
    CallError::Rpc(RpcError::new("local_lane_running", data, 1))
}

/// A `remote.use` reply that switched `acme@0` to `fork-b`.
fn switched() -> Value {
    json!({
        "lane": "acme@0", "host": "fork-b", "previous": null,
        "mirrored": true, "pending": false, "waits_for": null,
        "nothing_to_mirror": false, "replaced": [],
    })
}

/// The status reply, with lane `acme@0` `running` or `idle`.
fn status(running: bool) -> Value {
    json!({
        "items": [{
            "reference": "acme@0", "project": "acme", "index": 0, "branch": "main",
            "path": "/work/acme", "state": if running { "running" } else { "idle" },
            "config_changed": false, "units": [],
        }],
        "truncated": false,
        "next_cursor": null,
    })
}

/// A daemon whose view has one lane, running while `running` is set.
fn viewing(running: Arc<AtomicBool>) -> Scripted {
    Scripted::new(move |method, _, _| match method {
        Method::Status => Ok(status(running.load(Ordering::SeqCst))),
        Method::Hosts => Ok(json!({"items": []})),
        Method::HostList => Ok(json!({"items": [], "yielded_local_lanes": []})),
        _ => Ok(json!({})),
    })
}

/// A daemon event with no data.
fn daemon_event(name: &str, data: Value) -> DaemonEvent {
    DaemonEvent {
        seq: 0,
        event: name.into(),
        data,
    }
}

#[tokio::test]
async fn hello_must_come_first() {
    let mut port = Port::open(Scripted::new(|_, _, _| Ok(json!({}))));
    port.send(json!({"id": 1, "method": "watch"})).await;
    assert_eq!(
        port.recv().await,
        json!({"id": 1, "error": {"code": "hello_required", "message": HostError::hello_required().message}})
    );
    port.send(json!({"id": 2, "method": "hello", "params": {"protocol": 2, "extension": "9.0.0"}}))
        .await;
    let mismatch = port.recv().await;
    assert_eq!(mismatch["error"]["code"], "protocol_mismatch");
    assert_eq!(mismatch["error"]["data"], json!({"supported": [1]}));
    port.send(json!({"id": 3, "method": "hello", "params": {"protocol": 1, "extension": "0.1.0"}}))
        .await;
    assert_eq!(
        port.recv().await,
        json!({"id": 3, "result": {"protocol": 1, "devlanes": devlanes_version::VERSION, "daemon": "running"}})
    );
    port.send(json!({"id": 4, "method": "lane.clean", "params": {"lane": "acme@0"}}))
        .await;
    let unknown = port.recv().await;
    assert_eq!(unknown["id"], 4);
    assert_eq!(unknown["error"]["code"], "unknown_method");
    // Without a readable id nothing could match a reply, so none comes.
    port.to_host.write_all(&2u32.to_ne_bytes()).await.ok();
    port.to_host.write_all(b"{]").await.ok();
    port.send(json!({"id": "5", "method": "watch"})).await;
    assert!(port.quiet(Duration::from_secs(1)).await);
    port.send(json!({"id": 6, "method": "use", "params": {"lane": "acme@0"}}))
        .await;
    let invalid = port.recv().await;
    assert_eq!(invalid["id"], 6);
    assert_eq!(invalid["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn the_host_exits_cleanly_at_end_of_input() {
    let port = Port::open(Scripted::new(|_, _, _| Ok(json!({}))));
    drop(port.to_host);
    let ended = timeout(Duration::from_secs(5), port.session).await;
    assert!(matches!(ended, Ok(Ok(Ok(())))));
}

#[tokio::test]
async fn an_oversized_request_closes_the_port() {
    let mut port = Port::open(Scripted::new(|_, _, _| Ok(json!({}))));
    port.to_host
        .write_all(&((frame::MAX_IN + 1) as u32).to_ne_bytes())
        .await
        .ok();
    let ended = timeout(Duration::from_secs(5), port.session).await;
    assert!(matches!(ended, Ok(Ok(Err(FrameError::TooLargeIn(_))))));
}

#[tokio::test]
async fn use_keeps_the_refusal_and_builds_its_commands() {
    let daemon = Scripted::new(|method, _, _| match method {
        Method::RemoteUse => Err(refused(json!({"compose_project": "acme-lane0"}))),
        _ => Ok(json!({})),
    });
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    port.send(json!({"id": 2, "method": "use", "params": {"lane": "acme@0", "box": "fork-b"}}))
        .await;
    let reply = port.recv().await;
    assert_eq!(reply["error"]["code"], "local_lane_running");
    assert_eq!(
        reply["error"]["data"],
        json!({
            "holds": "processes",
            "local_lane": "acme@0",
            "commands": ["devlanes stop --lane acme@0", "docker compose -p acme-lane0 stop"],
        })
    );
    assert_eq!(
        daemon.calls(Method::RemoteUse),
        [(json!({"lane": "acme@0", "host": "fork-b"}), None)]
    );
}

#[tokio::test(start_paused = true)]
async fn stop_and_use_stops_the_local_lane_and_asks_again() {
    let daemon = Scripted::new(|method, _, before| match method {
        // The daemon counts the lane as running for a few seconds after the stop.
        Method::RemoteUse if before < 4 => Err(refused(json!({}))),
        Method::RemoteUse => Ok(switched()),
        _ => Ok(json!({})),
    });
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    let begun = Instant::now();
    port.send(
        json!({"id": 2, "method": "stop_and_use", "params": {"lane": "acme@0", "box": "fork-b"}}),
    )
    .await;
    assert_eq!(
        port.recv().await,
        json!({"id": 2, "result": {
            "lane": "acme@0", "box": "fork-b", "previous": null,
            "mirrored": true, "pending": false, "waits_for": null,
        }})
    );
    let stops = daemon.calls(Method::Stop);
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].1.as_deref(), Some("acme@0"));
    assert_eq!(daemon.calls(Method::RemoteUse).len(), 5);
    assert!(begun.elapsed() >= Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn stop_and_use_gives_up_after_thirty_seconds() {
    let daemon = Scripted::new(|method, _, _| match method {
        Method::RemoteUse => Err(refused(json!({}))),
        _ => Ok(json!({})),
    });
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    let begun = Instant::now();
    port.send(
        json!({"id": 2, "method": "stop_and_use", "params": {"lane": "acme@0", "box": "fork-b"}}),
    )
    .await;
    let reply = port.recv().await;
    assert_eq!(reply["error"]["code"], "local_lane_running", "{reply}");
    assert_eq!(
        reply["error"]["data"]["commands"],
        json!(["devlanes stop --lane acme@0"])
    );
    assert!(begun.elapsed() >= SWITCH_WAIT);
    assert!(begun.elapsed() < SWITCH_WAIT + Duration::from_secs(2));
    assert_eq!(daemon.calls(Method::Stop).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_and_use_never_stops_containers() {
    // Processes and the lane's own containers: the processes stop, and the
    // containers are left to the person.
    let daemon = Scripted::new(|method, _, _| match method {
        Method::RemoteUse => Err(refused(json!({
            "compose_project": "acme-lane0",
            "containers": ["acme-cache"],
        }))),
        _ => Ok(json!({})),
    });
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    port.send(
        json!({"id": 2, "method": "stop_and_use", "params": {"lane": "acme@0", "box": "fork-b"}}),
    )
    .await;
    let reply = port.recv().await;
    assert_eq!(reply["error"]["code"], "containers_hold_ports", "{reply}");
    assert_eq!(
        reply["error"]["data"],
        json!({"commands": ["docker compose -p acme-lane0 stop", "docker stop acme-cache"]})
    );
    assert_eq!(daemon.calls(Method::Stop).len(), 1);
    assert_eq!(daemon.calls(Method::RemoteUse).len(), 1);

    // Containers alone: nothing to stop here.
    let daemon = Scripted::new(|method, _, _| match method {
        Method::RemoteUse => Err(refused(json!({
            "holds": "containers",
            "compose_project": "acme-lane0",
        }))),
        _ => Ok(json!({})),
    });
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    port.send(
        json!({"id": 2, "method": "stop_and_use", "params": {"lane": "acme@0", "box": "fork-b"}}),
    )
    .await;
    let reply = port.recv().await;
    assert_eq!(reply["error"]["code"], "containers_hold_ports", "{reply}");
    assert!(daemon.calls(Method::Stop).is_empty());
}

#[tokio::test(start_paused = true)]
async fn watch_sends_the_view_once_per_burst_and_only_on_change() {
    let running = Arc::new(AtomicBool::new(true));
    let daemon = viewing(running.clone());
    let feed = daemon.subscribable();
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    port.send(json!({"id": 2, "method": "watch"})).await;
    assert_eq!(port.recv().await, json!({"id": 2, "result": {}}));
    let view = port.recv().await;
    assert_eq!(view["event"], "view");
    assert_eq!(
        view["data"]["projects"][0]["lanes"][0]["sources"][0]["state"],
        "running"
    );

    // A burst that changes nothing: one refresh, and no view.
    for _ in 0..5 {
        feed.send(daemon_event(events::UNIT_CHANGED, json!({})))
            .await
            .ok();
    }
    assert!(port.quiet(Duration::from_secs(1)).await);
    assert_eq!(daemon.calls(Method::Status).len(), 2);

    // An event the view doesn't depend on: no refresh.
    feed.send(daemon_event(events::LOG, json!({}))).await.ok();
    assert!(port.quiet(Duration::from_secs(1)).await);
    assert_eq!(daemon.calls(Method::Status).len(), 2);

    running.store(false, Ordering::SeqCst);
    feed.send(daemon_event(events::LANE_CHANGED, json!({})))
        .await
        .ok();
    let view = port.recv().await;
    assert_eq!(
        view["data"]["projects"][0]["lanes"][0]["sources"][0]["state"],
        "idle"
    );
}

#[tokio::test(start_paused = true)]
async fn watch_forwards_notices_and_the_daemon_state() {
    let daemon = viewing(Arc::new(AtomicBool::new(true)));
    let feed = daemon.subscribable();
    let mut port = Port::open(daemon.clone());
    port.hello().await;
    port.send(json!({"id": 2, "method": "watch"})).await;
    port.recv().await;
    port.recv().await;

    feed
        .send(daemon_event(
            remote::WARNING_EVENT,
            json!({"host": "fork-b", "code": "lane_switched", "lane": "acme@0", "to": "fork-b", "from": null, "message": "acme@0 now comes from fork-b"}),
        ))
        .await
        .ok();
    assert_eq!(
        port.recv().await,
        json!({"event": "notice", "data": {
            "code": "lane_switched", "lane": "acme@0", "from": null, "to": "fork-b",
            "message": "acme@0 now comes from fork-b",
        }})
    );
    feed.send(daemon_event(
        remote::WARNING_EVENT,
        json!({"host": "fork-b", "code": "lane_skipped", "lane": "acme@0", "message": "skipped"}),
    ))
    .await
    .ok();
    assert!(port.quiet(Duration::from_secs(1)).await);

    feed.send(daemon_event(events::DAEMON_STOPPING, json!({})))
        .await
        .ok();
    assert_eq!(
        port.recv().await,
        json!({"event": "daemon", "data": {"state": "stopped", "message": daemon::STOPPED}})
    );
    // The daemon is gone: the reconnects every 2 s report nothing new.
    drop(feed);
    assert!(port.quiet(Duration::from_secs(5)).await);

    // It comes back.
    let feed = daemon.subscribable();
    assert_eq!(
        port.recv().await,
        json!({"event": "daemon", "data": {"state": "running"}})
    );
    drop(feed);
}

/// `value` as JSON, for comparing with a fixture.
fn encoded(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the message encodes")
}

/// A daemon refusal of `remote.use` with `code` and its own `message`.
fn refusal(code: &str, message: &str) -> CallError {
    CallError::Rpc(RpcError::new(code, json!({"message": message}), 1))
}

/// `messages.json` holds what this host sends, built by the code that sends
/// it, and requests it reads. The browser repo tests its parsers on a copy.
#[test]
fn the_messages_fixture_matches_the_host() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/messages.json")).expect("JSON");
    for request in fixture["requests"].as_array().into_iter().flatten() {
        assert!(
            Request::parse(request.to_string().as_bytes()).is_ok(),
            "{request}"
        );
    }

    let hello = |daemon| HelloResult {
        protocol: 1,
        devlanes: "0.15.0".into(),
        daemon,
    };
    let to_box = UseParams {
        lane: "acme@0".into(),
        host: Some("fork-b".into()),
    };
    let to_local = UseParams {
        lane: "acme@0".into(),
        host: None,
    };
    let replies = [
        encoded(Reply {
            id: 1,
            result: hello(DaemonState::Running),
        }),
        encoded(Reply {
            id: 1,
            result: hello(DaemonState::Stopped),
        }),
        encoded(Reply {
            id: 2,
            result: WatchResult {},
        }),
        encoded(Reply {
            id: 3,
            result: use_result(
                &to_box,
                &json!({"lane": "acme@0", "host": "fork-b", "previous": "fork-a", "mirrored": true, "pending": false, "waits_for": null, "nothing_to_mirror": false, "replaced": []}),
            ),
        }),
        encoded(Reply {
            id: 4,
            result: use_result(
                &to_local,
                &json!({"lane": "acme@0", "host": null, "previous": "fork-b", "mirrored": true, "pending": false, "waits_for": null, "nothing_to_mirror": false, "replaced": []}),
            ),
        }),
        encoded(Reply {
            id: 5,
            result: use_result(
                &to_box,
                &json!({"lane": "acme@0", "host": "fork-b", "previous": null, "mirrored": false, "pending": true, "waits_for": "connect", "nothing_to_mirror": false, "replaced": []}),
            ),
        }),
    ];
    assert_eq!(fixture["replies"], json!(replies));

    let notice = |data: Value| Notice::of_warning(&data).map(Event::Notice);
    let events = [
        encoded(Event::View(View::default())),
        encoded(Event::Daemon(running())),
        encoded(Event::Daemon(unavailable_status(&Unavailable::Stopped(
            daemon::STOPPED.into(),
        )))),
        encoded(Event::Daemon(unavailable_status(&Unavailable::Outdated))),
        encoded(notice(json!({
            "host": "fork-b", "code": "lane_switched", "lane": "acme@0",
            "to": "fork-b", "from": "fork-a", "message": "acme@0 now comes from fork-b",
        }))),
        encoded(notice(json!({
            "host": "fork-b", "code": "choice_cleared", "lane": "acme@0",
            "local_lane": "acme@0", "holds": "processes",
            "message": "acme@0 started on this machine, so fork-b's acme@0 is no longer mirrored or chosen.",
        }))),
    ];
    assert_eq!(fixture["events"], json!(events));

    let invalid = Request::parse(br#"{"id": 7, "method": "use", "params": {"lane": "acme@0"}}"#)
        .expect_err("a use without a box is invalid");
    let errors = [
        ErrorReply {
            id: 3,
            error: use_error(CallError::Rpc(RpcError::new(
                "local_lane_running",
                json!({
                    "local_lane": "acme@0",
                    "holds": "processes",
                    "message": "acme@0 runs on this machine. Stop it with devlanes stop --lane acme@0. Then run devlanes remote use fork-b acme@0.",
                }),
                1,
            ))),
        },
        ErrorReply {
            id: 5,
            error: containers_hold_ports(
                "acme@0",
                vec!["docker compose -p acme-lane0 stop".into()],
            ),
        },
        ErrorReply {
            id: 1,
            error: HostError::protocol_mismatch(2),
        },
        ErrorReply {
            id: 3,
            error: use_error(CallError::Rpc(RpcError::new(
                "invalid_reference",
                json!({"reference": "acme@"}),
                2,
            ))),
        },
        ErrorReply {
            id: 2,
            error: HostError::hello_required(),
        },
        ErrorReply {
            id: invalid.id.unwrap_or_default(),
            error: HostError::plain("invalid_request", invalid.message),
        },
        ErrorReply {
            id: 6,
            error: HostError::unknown_method("lane.clean"),
        },
        ErrorReply {
            id: 3,
            error: use_error(refusal(
                "box_unknown",
                "this machine knows no box called fork-b. Run devlanes remote ls to see the boxes",
            )),
        },
        ErrorReply {
            id: 3,
            error: use_error(refusal(
                "lane_not_on_box",
                "box fork-b has no lane acme@0. Run devlanes remote ls to see its lanes",
            )),
        },
        ErrorReply {
            id: 4,
            error: use_error(refusal(
                "local_lane_unknown",
                "this machine has no lane acme@0. Run devlanes remote use <box> acme@0 to choose a box",
            )),
        },
    ];
    assert_eq!(fixture["errors"], encoded(errors));
}
