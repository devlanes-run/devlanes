//! The host served over in-memory pipes and loopback TCP, driven by the
//! headless client and by hand-written protocol bytes, on tokio's paused
//! clock wherever timers matter.

mod connections;
mod input;
mod locks;
mod streams;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::BytesMut;
use devlanes_gui_client::{Client, ClientOptions, Event, Events, connect_stream};
use devlanes_gui_host::{
    ServeError, ServeOptions, TestPattern, TestPatternHandle, TestPatternOptions, serve,
    start_backend,
};
use devlanes_gui_proto::{ClientMessage, CodecId, CodecOffer, Hello, HostMessage, Role, Token};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The control token of every test host.
const TOKEN: &str = "0123456789abcdef";

/// Longest a test waits for anything. On the paused clock a missing event
/// makes time jump here at once, so a broken host fails fast.
const PATIENCE: Duration = Duration::from_secs(60);

/// A test-pattern host serving in-memory pipes.
struct Host {
    pattern: TestPatternHandle,
    pipes: mpsc::Sender<DuplexStream>,
    served: JoinHandle<Result<(), ServeError>>,
    shutdown: CancellationToken,
    activity: Arc<AtomicUsize>,
}

impl Host {
    /// A host of a pattern with `pattern`'s options and `tweak` applied to its own.
    async fn start(pattern: TestPatternOptions, tweak: impl FnOnce(&mut ServeOptions)) -> Self {
        let (backend, handle) = TestPattern::new(pattern);
        let backend = start_backend(backend).await.expect("the pattern starts");
        let activity = Arc::new(AtomicUsize::new(0));
        let counter = activity.clone();
        let mut options = ServeOptions::new("acme:desktop");
        options.control_token = Some(Token::new(TOKEN));
        options.on_activity = Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        tweak(&mut options);
        let shutdown = options.shutdown.clone();
        let (pipes, accepted) = mpsc::channel(8);
        let served = tokio::spawn(serve(accepted, backend, options));
        Self {
            pattern: handle,
            pipes,
            served,
            shutdown,
            activity,
        }
    }

    /// A host with the default pattern and options.
    async fn default() -> Self {
        Self::start(TestPatternOptions::default(), |_| {}).await
    }

    /// The client end of a new connection, whose buffer holds `buffer` bytes.
    async fn pipe_with(&self, buffer: usize) -> DuplexStream {
        let (ours, theirs) = tokio::io::duplex(buffer);
        self.pipes.send(theirs).await.expect("the host accepts");
        ours
    }

    /// The client end of a new connection.
    async fn pipe(&self) -> DuplexStream {
        self.pipe_with(4 << 20).await
    }

    /// A client connected with `options`.
    async fn connect(&self, options: ClientOptions) -> (Client, Events) {
        connect_stream(self.pipe().await, options)
            .await
            .expect("the host welcomes the client")
    }

    /// A viewer named `name` decoding `raw`.
    async fn viewer(&self, name: &str) -> (Client, Events) {
        self.connect(ClientOptions::new(Role::Viewer, name, name))
            .await
    }

    /// A tool named `name`.
    async fn tool(&self, name: &str) -> (Client, Events) {
        self.connect(ClientOptions::new(Role::Tool, name, name))
            .await
    }

    /// The daemon's control connection.
    async fn control(&self) -> (Client, Events) {
        let options =
            ClientOptions::new(Role::Control, "daemon", "devlanes").with_token(Token::new(TOKEN));
        self.connect(options).await
    }

    /// Stops the host and checks it shut down cleanly.
    async fn stop(self) {
        self.shutdown.cancel();
        let result = tokio::time::timeout(PATIENCE, self.served)
            .await
            .expect("the host stops")
            .expect("the host task did not panic");
        assert!(result.is_ok(), "{result:?}");
    }
}

/// Waits for the first event `pick` takes, skipping the others.
async fn until<T>(events: &mut Events, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
    let wait = async {
        while let Some(event) = events.next().await {
            if let Some(found) = pick(&event) {
                return found;
            }
        }
        panic!("the connection closed first");
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .expect("the event came")
}

/// Polls `check` until it holds, for effects no message reports.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let wait = async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    if tokio::time::timeout(PATIENCE, wait).await.is_err() {
        panic!("never happened: {what}");
    }
}

/// The next decoded frame, skipping other events.
async fn next_frame(events: &mut Events) -> devlanes_gui_client::DecodedFrame {
    until(events, |event| match event {
        Event::Frame(frame) => Some(frame.clone()),
        _ => None,
    })
    .await
}

/// A connection spoken by hand, for what the client never sends.
struct Raw {
    stream: DuplexStream,
    buf: BytesMut,
}

impl Raw {
    /// Sends a hello with `role` and `token` over a new pipe, without reading the answer.
    async fn hello(host: &Host, role: Role, token: Option<&str>) -> Self {
        let decoders = match role {
            Role::Viewer => vec![CodecOffer {
                codec: CodecId::RAW,
                hardware: false,
            }],
            _ => Vec::new(),
        };
        let mut raw = Self {
            stream: host.pipe().await,
            buf: BytesMut::new(),
        };
        raw.send(&ClientMessage::Hello(Hello {
            role,
            client_id: "raw".into(),
            client_name: "raw".into(),
            decoders,
            features: Vec::new(),
            token: token.map(Token::new),
        }))
        .await;
        raw
    }

    /// A connection that said hello with `role` and got its welcome.
    async fn welcomed(host: &Host, role: Role) -> Self {
        let token = (role == Role::Control).then_some(TOKEN);
        let mut raw = Self::hello(host, role, token).await;
        let answer = raw.answer().await;
        assert!(matches!(answer, HostMessage::Welcome(_)), "{answer:?}");
        raw
    }

    /// Writes `message`; a host that closed the pipe is not an error here.
    async fn send(&mut self, message: &ClientMessage) {
        let mut out = BytesMut::new();
        message.encode(&mut out).expect("encodes");
        let _ = self.stream.write_all(&out).await;
    }

    /// The host's answer to the hello.
    async fn answer(&mut self) -> HostMessage {
        let wait = async {
            loop {
                if let Some(answer) = HostMessage::decode_answer(&mut self.buf).expect("an answer")
                {
                    return answer;
                }
                let read = self.stream.read_buf(&mut self.buf).await.expect("reads");
                assert!(read > 0, "the host closed without answering");
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .expect("answered")
    }

    /// The next message, or `None` once the host closed the connection.
    async fn read(&mut self) -> Option<HostMessage> {
        loop {
            if let Some(message) = HostMessage::decode(&mut self.buf).expect("a message") {
                return Some(message);
            }
            match self.stream.read_buf(&mut self.buf).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// Waits until the host handled everything sent before, through the
    /// answer to a screenshot of a window that does not exist.
    async fn round_trip(&mut self) {
        let request = 0xfeed;
        self.send(&ClientMessage::Screenshot {
            request,
            window: Some(devlanes_gui_proto::WindowId(u32::MAX)),
        })
        .await;
        let wait = async {
            loop {
                match self.read().await {
                    Some(HostMessage::Screenshot { request: r, .. }) if r == request => return,
                    Some(_) => {}
                    None => panic!("the host closed the connection"),
                }
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .expect("answered");
    }

    /// Reads until the host closes the connection, and returns what came.
    async fn closed(&mut self) -> Vec<HostMessage> {
        let wait = async {
            let mut got = Vec::new();
            while let Some(message) = self.read().await {
                got.push(message);
            }
            got
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .expect("the host closed the connection")
    }
}
