//! The host task: one owner for windows, streams, sessions and input, fed
//! by the connections, the backend and the encoders, and woken by a timer
//! for whatever the state machines say happens next.

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use devlanes_gui_codec::{
    CodecError, EncodeParams, EncoderBackend, Frame, RawEncoder, VideoEncoder,
};
use devlanes_gui_proto::{
    Buttons, ClientMessage, CodecId, CodecOffer, EncodeError, Encode, HEARTBEAT_INTERVAL, Hello,
    Holder, HostLock, HostMessage, InputArbiter, Key, LockState, MAX_APP_ID, MAX_CLIPBOARD,
    MAX_DIMENSION, MAX_TITLE, MAX_WINDOWS, Pacer, Point, Pointer, RESIZE_TIMEOUT, Released, Role,
    SYNC_FEATURE, Screenshot, ScreenshotResult, SessionId, Size, Verdict, VideoFrame, Welcome,
    WindowId, WindowInfo, WindowKind,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::backend::{BackendControl, CapturedFrame, Command, Event};
use crate::outbox::Outbox;
use crate::{ActivityCallback, MAX_SCREENSHOTS_IN_FLIGHT, SYNC_TIMEOUT, ServeError, ServeOptions};

/// Resize requests kept per window while the backend has not answered; the
/// oldest goes first, and its viewer ends Resizing on its timeout.
const MAX_PENDING_RESIZES: usize = 64;

/// What the connections send the host task.
pub(crate) enum Inbox {
    /// A hello passed its checks.
    Join {
        hello: Hello,
        /// A viewer's codec.
        codec: Option<CodecOffer>,
        outbox: Arc<Outbox>,
        reply: oneshot::Sender<SessionId>,
    },
    /// A message the session's role may send.
    Message {
        session: SessionId,
        message: ClientMessage,
    },
    /// The connection ended.
    Leave { session: SessionId },
}

/// The current time on tokio's clock, which tests pause and advance.
fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

/// One window and one codec.
type StreamKey = (WindowId, CodecOffer);

/// One connection.
struct Session {
    role: Role,
    client_id: String,
    name: String,
    outbox: Arc<Outbox>,
    /// A viewer's codec.
    codec: Option<CodecOffer>,
    /// When the last message was queued, for heartbeats.
    last_sent: Instant,
    /// The window its last input went to, which `Sync` waits on.
    last_input_window: Option<WindowId>,
    screenshots: usize,
}

/// One window as viewers see it. Its id is the one viewers know, which stays
/// the same when a new window of the app takes the place of one that closed.
struct Window {
    info: WindowInfo,
    /// The backend's id of the window now in this place.
    backend_id: WindowId,
    lock: HostLock,
    /// The state viewers were last told.
    sent_lock: LockState,
    /// The last frame captured, for keyframes and screenshots.
    frame: Option<Frame>,
    /// When the last size change was applied.
    resized_at: Option<Instant>,
    /// Whether `frame` was captured after that change.
    frame_after_resize: bool,
    /// A size change waits for its first frame to be queued to every viewer.
    awaiting_frame: bool,
    /// Closed while the service runs; waiting for the app to open another.
    gone: bool,
}

impl Window {
    /// Whether it is a main window.
    fn is_toplevel(&self) -> bool {
        self.info.kind == WindowKind::Toplevel && self.info.parent.is_none()
    }
}

/// One window's frames in one codec, shared by the viewers that chose it.
struct Stream {
    /// Tells a result of an earlier stream with the same key from one of this.
    generation: u64,
    pacer: Pacer<Frame>,
    backend: Arc<dyn EncoderBackend>,
    /// Away on the blocking pool while `busy`; created there on first use.
    encoder: Option<Box<dyn VideoEncoder>>,
    busy: bool,
    failed: bool,
    viewers: Vec<SessionId>,
    /// The first sequence number after a size change, until a frame from it on is queued.
    awaiting_from: Option<u64>,
    /// When to give a new stream the window's cached frame, which a recent
    /// resize holds back.
    seed_at: Option<Instant>,
}

/// A resize request the backend has not answered.
struct PendingResize {
    id: u64,
    window: WindowId,
    session: SessionId,
    request: u32,
}

/// A `Sync` waiting for its frame.
struct PendingSync {
    id: u64,
    window: WindowId,
    session: SessionId,
    request: u32,
    deadline: Instant,
}

/// Why a frame was not encoded.
#[derive(Debug, thiserror::Error)]
enum EncodeFailure {
    #[error("the encoder failed")]
    Codec(#[from] CodecError),
    #[error("the frame message could not be written")]
    Message(#[from] EncodeError),
    #[error("the encoder panicked")]
    Panicked,
}

/// Work finished on the blocking pool.
enum Done {
    Encoded {
        key: StreamKey,
        generation: u64,
        seq: u64,
        recipients: Vec<SessionId>,
        encoder: Option<Box<dyn VideoEncoder>>,
        /// The encoded frame message, or `None` when the picture did not change.
        result: Result<Option<Bytes>, EncodeFailure>,
    },
    Screenshot {
        session: SessionId,
        message: Bytes,
    },
}

/// The host task's state.
pub(crate) struct Hub {
    service: String,
    real_desktop: bool,
    encoders: Vec<Arc<dyn EncoderBackend>>,
    on_activity: Option<ActivityCallback>,
    shutdown: CancellationToken,
    backend: Box<dyn BackendControl>,
    sessions: BTreeMap<SessionId, Session>,
    windows: BTreeMap<WindowId, Window>,
    /// The backend's window ids to the ids viewers know.
    by_backend: HashMap<WindowId, WindowId>,
    streams: HashMap<StreamKey, Stream>,
    arbiter: InputArbiter,
    holder: Option<SessionId>,
    resizes: Vec<PendingResize>,
    syncs: Vec<PendingSync>,
    tasks: JoinSet<Done>,
    next_session: u64,
    next_id: u64,
    next_generation: u64,
    last_activity: Option<Instant>,
    frozen: bool,
    stopped: bool,
    /// The display's buttons as last sent to the backend.
    buttons: Buttons,
    /// Where the last accepted pointer message was, in backend ids, for releases.
    last_pointer: Option<(WindowId, Point)>,
    /// The window the last accepted key went to, in backend ids, for releases.
    last_key_window: Option<WindowId>,
    /// Sessions to close at the end of this turn.
    closing: Vec<SessionId>,
    heartbeat: Bytes,
}

impl Hub {
    /// A host task for `backend`.
    pub(crate) fn new(
        backend: Box<dyn BackendControl>,
        options: &ServeOptions,
        real_desktop: bool,
    ) -> Self {
        let mut heartbeat = BytesMut::new();
        // A heartbeat has no fields, so encoding cannot fail.
        let _ = HostMessage::Heartbeat.encode(&mut heartbeat);
        Self {
            service: options.service.clone(),
            real_desktop,
            encoders: options.encoders.clone(),
            on_activity: options.on_activity.clone(),
            shutdown: options.shutdown.clone(),
            backend,
            sessions: BTreeMap::new(),
            windows: BTreeMap::new(),
            by_backend: HashMap::new(),
            streams: HashMap::new(),
            arbiter: InputArbiter::new(),
            holder: None,
            resizes: Vec::new(),
            syncs: Vec::new(),
            tasks: JoinSet::new(),
            next_session: 1,
            next_id: 1,
            next_generation: 1,
            last_activity: None,
            frozen: false,
            stopped: false,
            buttons: Buttons::NONE,
            last_pointer: None,
            last_key_window: None,
            closing: Vec::new(),
            heartbeat: heartbeat.freeze(),
        }
    }

    /// Runs until shutdown or until the backend stops.
    pub(crate) async fn run(
        mut self,
        mut inbox: mpsc::Receiver<Inbox>,
        mut events: mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), ServeError> {
        let shutdown = self.shutdown.clone();
        loop {
            let wake = self.next_wake(now());
            let timer = async {
                match wake {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = shutdown.cancelled() => {
                    self.close_all();
                    return Ok(());
                }
                event = events.recv() => match event {
                    Some(event) => self.on_event(event),
                    None => {
                        self.backend_gone();
                        return Err(ServeError::BackendStopped);
                    }
                },
                message = inbox.recv() => match message {
                    Some(message) => self.on_inbox(message),
                    None => {
                        self.close_all();
                        return Ok(());
                    }
                },
                Some(done) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    // Jobs catch their own panics, so a join error is a cancelled task at shutdown.
                    if let Ok(done) = done {
                        self.on_done(done);
                    }
                }
                _ = timer => {}
            }
            self.tick();
        }
    }

    /// The earliest time a timer-driven change is due.
    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let streams = self
            .streams
            .values()
            .filter(|s| !s.busy && !s.failed)
            .flat_map(|s| [s.pacer.wake_at(now), s.seed_at]);
        let windows = self.windows.values().map(|w| w.lock.next_change(now));
        let heartbeats = self
            .sessions
            .values()
            .map(|s| Some(s.last_sent + HEARTBEAT_INTERVAL));
        let syncs = self.syncs.iter().map(|s| Some(s.deadline));
        streams
            .chain(windows)
            .chain(heartbeats)
            .chain(syncs)
            .chain([self.arbiter.next_change(now)])
            .flatten()
            .min()
    }

    /// Everything that follows from time passing or the last change.
    fn tick(&mut self) {
        let now = now();
        let released = self.arbiter.expire(now);
        self.release(released);
        self.refresh_holder(now);
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.syncs)
            .into_iter()
            .partition(|s| s.deadline <= now);
        self.syncs = waiting;
        for sync in due {
            self.send(sync.session, &HostMessage::Synced {
                request: sync.request,
            });
        }
        for ((window, _), stream) in &mut self.streams {
            if stream.seed_at.is_some_and(|at| at <= now) {
                stream.seed_at = None;
                if let Some(frame) = self.windows.get(window).and_then(|w| w.frame.clone()) {
                    stream.pacer.offer(frame);
                }
            }
        }
        self.refresh_locks(now);
        self.pump(now);
        let heartbeat = self.heartbeat.clone();
        let due: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| now >= s.last_sent + HEARTBEAT_INTERVAL)
            .map(|(id, _)| *id)
            .collect();
        for session in due {
            self.push(session, heartbeat.clone());
        }
        for session in std::mem::take(&mut self.closing) {
            self.leave(session);
        }
    }

    /// Queues `message` for `session`; one past the write cap closes it.
    fn push(&mut self, session: SessionId, message: Bytes) {
        let Some(s) = self.sessions.get_mut(&session) else {
            return;
        };
        match s.outbox.push(message) {
            Ok(()) => s.last_sent = now(),
            Err(_) => {
                tracing::debug!(session = session.0, "closing a GUI connection whose write queue is full");
                self.closing.push(session);
            }
        }
    }

    /// Encodes `message` and queues it for `session`.
    fn send(&mut self, session: SessionId, message: &HostMessage) {
        if let Some(bytes) = encode(message) {
            self.push(session, bytes);
        }
    }

    /// Encodes `message` once and queues it for every session whose role passes `to`.
    fn broadcast(&mut self, to: fn(Role) -> bool, message: &HostMessage) {
        let Some(bytes) = encode(message) else {
            return;
        };
        let targets: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| to(s.role))
            .map(|(id, _)| *id)
            .collect();
        for session in targets {
            self.push(session, bytes.clone());
        }
    }

    /// Handles a message from a connection.
    fn on_inbox(&mut self, message: Inbox) {
        match message {
            Inbox::Join {
                hello,
                codec,
                outbox,
                reply,
            } => self.join(hello, codec, outbox, reply),
            Inbox::Message { session, message } => self.on_message(session, message),
            Inbox::Leave { session } => self.leave(session),
        }
    }

    /// Welcomes a connection and, for a viewer, adds it to every window's stream.
    fn join(
        &mut self,
        hello: Hello,
        codec: Option<CodecOffer>,
        outbox: Arc<Outbox>,
        reply: oneshot::Sender<SessionId>,
    ) {
        let session = SessionId(self.next_session);
        self.next_session += 1;
        let welcome = HostMessage::Welcome(Welcome {
            session,
            service: self.service.clone(),
            encoders: self.encoders.iter().map(|e| e.offer()).collect(),
            real_desktop: self.real_desktop,
            windows: self.windows.values().map(|w| w.info.clone()).collect(),
            features: vec![SYNC_FEATURE.to_owned()],
        });
        let Some(welcome) = encode(&welcome) else {
            return;
        };
        if outbox.push(welcome).is_err() {
            return;
        }
        self.sessions.insert(
            session,
            Session {
                role: hello.role,
                client_id: hello.client_id,
                name: hello.client_name,
                outbox,
                codec,
                last_sent: now(),
                last_input_window: None,
                screenshots: 0,
            },
        );
        if hello.role == Role::Viewer {
            let windows: Vec<_> = self.windows.keys().copied().collect();
            for window in windows {
                self.watch(window, session);
            }
        }
        if matches!(hello.role, Role::Viewer | Role::Tool) && self.holder.is_some() {
            let notice = HostMessage::InputHolder(self.holder_notice());
            self.send(session, &notice);
        }
        let _ = reply.send(session);
    }

    /// Forgets a session: its streams, what it held, what it waited for.
    fn leave(&mut self, session: SessionId) {
        let Some(gone) = self.sessions.remove(&session) else {
            return;
        };
        gone.outbox.close(false);
        let mut emptied = Vec::new();
        for (key, stream) in &mut self.streams {
            if stream.viewers.contains(&session) {
                stream.viewers.retain(|v| *v != session);
                stream.pacer.remove_viewer(session);
                if stream.viewers.is_empty() {
                    emptied.push(*key);
                }
            }
        }
        for key in emptied {
            self.streams.remove(&key);
            self.check_resize_done(key.0);
        }
        let released = self.arbiter.remove(session);
        self.release(vec![(session, released)]);
        self.resizes.retain(|r| r.session != session);
        self.syncs.retain(|s| s.session != session);
    }

    /// Adds viewer `session` to the stream of `window` in its codec,
    /// starting the stream with the cached frame when there is one.
    fn watch(&mut self, window: WindowId, session: SessionId) {
        let Some(codec) = self.sessions.get(&session).and_then(|s| s.codec) else {
            return;
        };
        let Some(win) = self.windows.get(&window) else {
            return;
        };
        let key = (window, codec);
        if !self.streams.contains_key(&key) {
            let Some(backend) = self.encoders.iter().find(|e| e.offer() == codec).cloned() else {
                return;
            };
            let mut stream = Stream {
                generation: self.next_generation,
                pacer: Pacer::new(),
                backend,
                encoder: None,
                busy: false,
                failed: false,
                viewers: Vec::new(),
                awaiting_from: None,
                seed_at: None,
            };
            self.next_generation += 1;
            if let Some(frame) = win.frame.clone() {
                let held_back = win
                    .resized_at
                    .filter(|_| !win.frame_after_resize)
                    .map(|at| at + RESIZE_TIMEOUT)
                    .filter(|until| now() < *until);
                match held_back {
                    Some(until) => stream.seed_at = Some(until),
                    None => stream.pacer.offer(frame),
                }
            }
            self.streams.insert(key, stream);
        }
        if let Some(stream) = self.streams.get_mut(&key) {
            stream.viewers.push(session);
            stream.pacer.add_viewer(session);
        }
    }

    /// The stream `session` watches `window` in.
    fn stream_of(&mut self, session: SessionId, window: WindowId) -> Option<&mut Stream> {
        let codec = self.sessions.get(&session)?.codec?;
        self.streams.get_mut(&(window, codec))
    }

    /// Handles a message the session's role may send.
    fn on_message(&mut self, session: SessionId, message: ClientMessage) {
        if !self.sessions.contains_key(&session) {
            return;
        }
        let now = now();
        match message {
            ClientMessage::FrameAck { window, seq } => {
                self.arbiter.alive(session, now);
                if let Some(stream) = self.stream_of(session, window) {
                    stream.pacer.ack(session, seq, now);
                }
            }
            ClientMessage::KeyframeRequest { window } => {
                self.arbiter.alive(session, now);
                if let Some(stream) = self.stream_of(session, window) {
                    stream.pacer.request_keyframe(session);
                }
            }
            ClientMessage::KeepAlive => self.arbiter.alive(session, now),
            ClientMessage::Screenshot { request, window } => {
                self.arbiter.alive(session, now);
                self.screenshot(session, request, window);
            }
            ClientMessage::Sync { request } => {
                self.arbiter.alive(session, now);
                self.sync(session, request, now);
            }
            ClientMessage::UnitProcess {
                pid,
                start_time,
                job,
            } => self.backend.send(Command::UnitIdentity {
                pid,
                start_time,
                job,
            }),
            ClientMessage::UnitExited { code } => {
                self.backend.send(Command::UnitExited { code });
                self.stopped = true;
                for window in self.windows.values_mut() {
                    window.lock.service_stopped();
                }
            }
            ClientMessage::LaneFrozen | ClientMessage::LaneThawed => {
                let frozen = matches!(message, ClientMessage::LaneFrozen);
                self.frozen = frozen;
                for window in self.windows.values_mut() {
                    window.lock.set_frozen(frozen);
                }
                self.backend.send(match frozen {
                    true => Command::Frozen,
                    false => Command::Thawed,
                });
            }
            // The reader never forwards a hello.
            ClientMessage::Hello(_) => {}
            input => self.input(session, input, now),
        }
    }

    /// The backend id of `window` if input may reach it now.
    fn input_target(&self, window: WindowId, now: Instant) -> Option<WindowId> {
        let win = self.windows.get(&window)?;
        (!win.gone && !win.lock.state(now).is_locked()).then_some(win.backend_id)
    }

    /// Handles input from a viewer or tool.
    fn input(&mut self, session: SessionId, message: ClientMessage, now: Instant) {
        self.activity(now);
        let released = self.arbiter.expire(now);
        self.release(released);
        let window = match &message {
            ClientMessage::Pointer(p) => Some(p.window),
            ClientMessage::Wheel(w) => Some(w.window),
            ClientMessage::Key(k) => Some(k.window),
            ClientMessage::PointerLeft { window }
            | ClientMessage::Text { window, .. }
            | ClientMessage::Focus { window, .. }
            | ClientMessage::Resize { window, .. }
            | ClientMessage::Close { window } => Some(*window),
            _ => None,
        };
        if let (Some(window), Some(s)) = (window, self.sessions.get_mut(&session)) {
            s.last_input_window = Some(window);
        }
        match message {
            ClientMessage::Resize {
                window,
                size,
                request,
            } => return self.resize(session, window, size, request),
            ClientMessage::Close { window } => {
                if let Some(win) = self.windows.get(&window).filter(|w| !w.gone) {
                    self.backend.send(Command::Close {
                        window: win.backend_id,
                    });
                }
                return;
            }
            ClientMessage::Clipboard { text } => {
                return self.backend.send(Command::Clipboard { text });
            }
            _ => {}
        }
        let Some(target) = window.and_then(|w| self.input_target(w, now)) else {
            self.arbiter.alive(session, now);
            return;
        };
        let command = match message {
            ClientMessage::Pointer(pointer) => {
                match self.arbiter.pointer(session, pointer.buttons, now) {
                    Verdict::Accept => {
                        self.buttons = pointer.buttons;
                        self.last_pointer = Some((target, pointer.position));
                        Some(Command::Pointer(Pointer {
                            window: target,
                            ..pointer
                        }))
                    }
                    Verdict::Drop => None,
                }
            }
            ClientMessage::Key(key) => match self.arbiter.key(session, key.code, key.down, now) {
                Verdict::Accept => {
                    self.last_key_window = Some(target);
                    Some(Command::Key(Key {
                        window: target,
                        ..key
                    }))
                }
                Verdict::Drop => None,
            },
            ClientMessage::Text { text, .. } => match self.arbiter.text(session, now) {
                Verdict::Accept => Some(Command::Text {
                    window: target,
                    text,
                }),
                Verdict::Drop => None,
            },
            ClientMessage::Wheel(wheel) => (self.arbiter.wheel(session, now) == Verdict::Accept)
                .then_some(Command::Wheel(devlanes_gui_proto::Wheel {
                    window: target,
                    ..wheel
                })),
            ClientMessage::PointerLeft { .. } => (self.arbiter.wheel(session, now)
                == Verdict::Accept)
                .then_some(Command::PointerLeft { window: target }),
            ClientMessage::Focus { focused, .. } => (self.arbiter.wheel(session, now)
                == Verdict::Accept)
                .then_some(Command::Focus {
                    window: target,
                    focused,
                }),
            _ => None,
        };
        if let Some(command) = command {
            self.backend.send(command);
        }
        self.refresh_holder(now);
    }

    /// Releases on the backend what silent or departed clients held.
    fn release(&mut self, released: Vec<(SessionId, Released)>) {
        for (_, held) in released {
            if !held.buttons.is_empty()
                && let Some((window, position)) = self.last_pointer
            {
                self.buttons = Buttons(self.buttons.0 & !held.buttons.0);
                self.backend.send(Command::Pointer(Pointer {
                    window,
                    position,
                    buttons: self.buttons,
                    screen_delta: None,
                }));
            }
            if let Some(window) = self.last_key_window {
                for code in held.keys {
                    self.backend.send(Command::Key(Key {
                        window,
                        code,
                        down: false,
                    }));
                }
            }
        }
    }

    /// Tells viewers and tools when the input holder changed.
    fn refresh_holder(&mut self, now: Instant) {
        let holder = self.arbiter.holder(now);
        if holder != self.holder {
            self.holder = holder;
            let notice = HostMessage::InputHolder(self.holder_notice());
            self.broadcast(sees_windows, &notice);
        }
    }

    /// The notice naming the current holder.
    fn holder_notice(&self) -> Option<Holder> {
        let session = self.holder?;
        let s = self.sessions.get(&session)?;
        Some(Holder {
            session,
            client_id: s.client_id.clone(),
            name: s.name.clone(),
        })
    }

    /// Input counts as lane activity, reported at most once per heartbeat interval.
    fn activity(&mut self, now: Instant) {
        if self
            .last_activity
            .is_some_and(|at| now < at + HEARTBEAT_INTERVAL)
        {
            return;
        }
        self.last_activity = Some(now);
        if let Some(callback) = &self.on_activity {
            callback();
        }
        self.broadcast(|role| role == Role::Control, &HostMessage::Activity);
    }

    /// Forwards a resize request and remembers whom to answer.
    fn resize(&mut self, session: SessionId, window: WindowId, size: Size, request: u32) {
        let Some(win) = self.windows.get(&window).filter(|w| !w.gone) else {
            return;
        };
        let backend_id = win.backend_id;
        let id = self.next_id;
        self.next_id += 1;
        let pending = self.resizes.iter().filter(|r| r.window == window).count();
        if pending >= MAX_PENDING_RESIZES
            && let Some(oldest) = self.resizes.iter().position(|r| r.window == window)
        {
            self.resizes.remove(oldest);
        }
        self.resizes.push(PendingResize {
            id,
            window,
            session,
            request,
        });
        self.backend.send(Command::Resize {
            window: backend_id,
            size,
            id,
        });
    }

    /// Answers a screenshot from the window's cached frame, encoded as a raw
    /// keyframe on the blocking pool.
    fn screenshot(&mut self, session: SessionId, request: u32, window: Option<WindowId>) {
        let Some(s) = self.sessions.get_mut(&session) else {
            return;
        };
        if s.screenshots >= MAX_SCREENSHOTS_IN_FLIGHT {
            tracing::debug!(session = session.0, "closing a GUI connection with too many screenshots in progress");
            self.closing.push(session);
            return;
        }
        let target = match window {
            Some(id) => self.windows.get(&id),
            None => self.windows.values().find(|w| w.is_toplevel() && !w.gone),
        };
        let (window, frame) = match target.filter(|w| !w.gone) {
            None => {
                let answer = HostMessage::Screenshot {
                    request,
                    result: ScreenshotResult::NoSuchWindow,
                };
                return self.send(session, &answer);
            }
            Some(win) => match &win.frame {
                None => {
                    let answer = HostMessage::Screenshot {
                        request,
                        result: ScreenshotResult::NoFrame,
                    };
                    return self.send(session, &answer);
                }
                Some(frame) => (win.info.id, frame.clone()),
            },
        };
        if let Some(s) = self.sessions.get_mut(&session) {
            s.screenshots += 1;
        }
        self.tasks.spawn_blocking(move || {
            let result = std::panic::catch_unwind(|| screenshot_result(window, &frame))
                .unwrap_or_else(|_| ScreenshotResult::Failed("the encoder panicked".to_owned()));
            let message = encode(&HostMessage::Screenshot { request, result })
                .unwrap_or_default();
            Done::Screenshot { session, message }
        });
    }

    /// Asks the backend for a frame of the window the session last sent
    /// input to, and answers once it arrives.
    fn sync(&mut self, session: SessionId, request: u32, now: Instant) {
        let last = self
            .sessions
            .get(&session)
            .and_then(|s| s.last_input_window);
        let window = last
            .and_then(|id| self.windows.get(&id))
            .filter(|w| !w.gone)
            .or_else(|| self.windows.values().find(|w| w.is_toplevel() && !w.gone));
        let Some(window) = window else {
            return self.send(session, &HostMessage::Synced { request });
        };
        let (shown, backend_id) = (window.info.id, window.backend_id);
        let id = self.next_id;
        self.next_id += 1;
        self.backend.send(Command::RequestFrame {
            window: backend_id,
            request: id,
        });
        self.syncs.push(PendingSync {
            id,
            window: shown,
            session,
            request,
            deadline: now + SYNC_TIMEOUT,
        });
    }

    /// Answers the syncs of `window` up to request `id`, or all of them.
    fn answer_syncs(&mut self, window: WindowId, id: Option<u64>) {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.syncs)
            .into_iter()
            .partition(|s| s.window == window && id.is_none_or(|id| s.id <= id));
        self.syncs = waiting;
        for sync in done {
            self.send(sync.session, &HostMessage::Synced {
                request: sync.request,
            });
        }
    }

    /// Handles an event from the backend.
    fn on_event(&mut self, event: Event) {
        match event {
            Event::WindowAdded(info) => self.window_added(info),
            Event::WindowChanged(info) => self.window_changed(info),
            Event::WindowRemoved(id) => self.window_removed(id),
            Event::Frame(frame) => self.frame(frame),
            Event::Resized { window, answers } => self.resized(window, answers),
            Event::Cursor(mut cursor) => {
                if let Some(id) = self.by_backend.get(&cursor.window) {
                    cursor.window = *id;
                    self.broadcast(is_viewer, &HostMessage::Cursor(cursor));
                }
            }
            Event::MoveResizeStarted(mut started) => {
                if let Some(id) = self.by_backend.get(&started.window) {
                    started.window = *id;
                    self.broadcast(is_viewer, &HostMessage::MoveResizeStarted(started));
                }
            }
            Event::Clipboard(mut text) => {
                truncate(&mut text, MAX_CLIPBOARD);
                self.broadcast(is_viewer, &HostMessage::Clipboard { text });
            }
        }
    }

    /// Fits a backend's description into the protocol's limits and the ids viewers know.
    fn adapt(&self, mut info: WindowInfo, id: WindowId) -> WindowInfo {
        truncate(&mut info.title, MAX_TITLE);
        truncate(&mut info.app_id, MAX_APP_ID);
        info.size = fit(info.size);
        info.min_size = info.min_size.map(fit);
        info.max_size = info.max_size.map(fit);
        info.parent = info
            .parent
            .map(|parent| self.by_backend.get(&parent).copied().unwrap_or(parent));
        info.id = id;
        info
    }

    /// A window appeared. A main window that opens while another waits for
    /// the app takes that one's place.
    fn window_added(&mut self, info: WindowInfo) {
        let backend_id = info.id;
        if self.by_backend.contains_key(&backend_id) {
            tracing::debug!(window = backend_id.0, "the GUI backend added a window twice");
            return;
        }
        let now = now();
        let toplevel = info.kind == WindowKind::Toplevel && info.parent.is_none();
        let waiting = self
            .windows
            .values()
            .find(|w| w.gone && w.is_toplevel())
            .map(|w| w.info.id)
            .filter(|_| toplevel);
        if let Some(shown) = waiting {
            let mut info = self.adapt(info, shown);
            self.by_backend.insert(backend_id, shown);
            let Some(win) = self.windows.get_mut(&shown) else {
                return;
            };
            win.backend_id = backend_id;
            win.gone = false;
            win.frame = None;
            win.lock.window_back();
            win.sent_lock = win.lock.state(now);
            info.lock = win.sent_lock;
            win.info = info.clone();
            return self.broadcast(sees_windows, &HostMessage::WindowChanged(info));
        }
        if self.windows.len() >= MAX_WINDOWS {
            tracing::warn!("the GUI backend has more windows than the protocol lists; ignoring one");
            return;
        }
        let mut lock = HostLock::new();
        lock.set_frozen(self.frozen);
        if self.stopped {
            lock.service_stopped();
        }
        let mut info = self.adapt(info, backend_id);
        info.lock = lock.state(now);
        self.by_backend.insert(backend_id, backend_id);
        self.windows.insert(
            backend_id,
            Window {
                info: info.clone(),
                backend_id,
                sent_lock: info.lock,
                lock,
                frame: None,
                resized_at: None,
                frame_after_resize: false,
                awaiting_frame: false,
                gone: false,
            },
        );
        self.broadcast(sees_windows, &HostMessage::WindowAdded(info));
        let viewers: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.role == Role::Viewer)
            .map(|(id, _)| *id)
            .collect();
        for viewer in viewers {
            self.watch(backend_id, viewer);
        }
    }

    /// A window's description changed.
    fn window_changed(&mut self, info: WindowInfo) {
        let Some(&shown) = self.by_backend.get(&info.id) else {
            return;
        };
        let mut info = self.adapt(info, shown);
        let Some(win) = self.windows.get_mut(&shown) else {
            return;
        };
        info.lock = win.sent_lock;
        win.info = info.clone();
        self.broadcast(sees_windows, &HostMessage::WindowChanged(info));
    }

    /// A window closed. The last main window of a running service waits for
    /// the app to open another; anything else goes at once.
    fn window_removed(&mut self, backend_id: WindowId) {
        let Some(shown) = self.by_backend.remove(&backend_id) else {
            return;
        };
        self.answer_syncs(shown, None);
        self.resizes.retain(|r| r.window != shown);
        let other_main = self
            .windows
            .values()
            .any(|w| w.info.id != shown && w.is_toplevel() && !w.gone);
        let Some(win) = self.windows.get_mut(&shown) else {
            return;
        };
        if win.is_toplevel() && !other_main && !self.stopped {
            win.gone = true;
            win.frame = None;
            win.lock.window_gone(now());
        } else {
            self.remove_window(shown);
        }
    }

    /// Forgets a window and tells viewers and tools.
    fn remove_window(&mut self, window: WindowId) {
        if let Some(win) = self.windows.remove(&window) {
            self.by_backend.remove(&win.backend_id);
        }
        self.streams.retain(|(id, _), _| *id != window);
        self.resizes.retain(|r| r.window != window);
        self.answer_syncs(window, None);
        self.broadcast(sees_windows, &HostMessage::WindowRemoved { window });
    }

    /// A frame arrived: cache it, offer it to the window's streams, and
    /// answer the syncs it was captured for.
    fn frame(&mut self, captured: CapturedFrame) {
        let Some(&shown) = self.by_backend.get(&captured.window) else {
            return;
        };
        let Some(win) = self.windows.get_mut(&shown) else {
            return;
        };
        win.frame = Some(captured.frame.clone());
        win.frame_after_resize = true;
        let mut streams = 0;
        for ((window, _), stream) in &mut self.streams {
            if *window == shown {
                stream.pacer.offer(captured.frame.clone());
                stream.seed_at = None;
                streams += 1;
            }
        }
        if streams == 0 && win.awaiting_frame {
            win.awaiting_frame = false;
            win.lock.frame_queued();
        }
        if let Some(id) = captured.answers {
            self.answer_syncs(shown, Some(id));
        }
    }

    /// The backend applied a size change: lock the window and tell every
    /// viewer from which sequence number its frames follow the change.
    fn resized(&mut self, backend_id: WindowId, answers: Option<u64>) {
        let Some(&shown) = self.by_backend.get(&backend_id) else {
            return;
        };
        let now = now();
        let Some(win) = self.windows.get_mut(&shown) else {
            return;
        };
        win.lock.resize_applied(now);
        win.resized_at = Some(now);
        win.frame_after_resize = false;
        win.awaiting_frame = true;
        // Requests are answered in order, so a merged change answers each
        // session with its newest request.
        let mut answered = HashMap::new();
        if let Some(id) = answers {
            self.resizes.retain(|r| {
                let covered = r.window == shown && r.id <= id;
                if covered {
                    answered.insert(r.session, r.request);
                }
                !covered
            });
        }
        let mut from = HashMap::new();
        for ((window, _), stream) in &mut self.streams {
            if *window != shown {
                continue;
            }
            let first = stream.pacer.resize_applied(now);
            stream.awaiting_from = Some(first);
            if stream.seed_at.is_some() {
                stream.seed_at = Some(now + RESIZE_TIMEOUT);
            }
            for viewer in &stream.viewers {
                from.insert(*viewer, first);
            }
        }
        self.refresh_locks(now);
        let targets: Vec<_> = self
            .sessions
            .iter()
            .map(|(id, s)| (*id, s.role))
            .collect();
        for (session, role) in targets {
            let request = answered.get(&session).copied();
            let from_seq = match role {
                Role::Viewer => from.get(&session).copied().unwrap_or(0),
                Role::Tool if request.is_some() => 0,
                _ => continue,
            };
            self.send(session, &HostMessage::ResizeApplied {
                window: shown,
                request: request.unwrap_or(0),
                from_seq,
            });
        }
    }

    /// Ends Resizing for `window` once none of its streams waits for a frame
    /// after the change.
    fn check_resize_done(&mut self, window: WindowId) {
        let waiting = self
            .streams
            .iter()
            .any(|((id, _), s)| *id == window && s.awaiting_from.is_some());
        if let Some(win) = self.windows.get_mut(&window)
            && win.awaiting_frame
            && !waiting
        {
            win.awaiting_frame = false;
            win.lock.frame_queued();
        }
    }

    /// Sends LockChanged for every window whose state changed, and forgets
    /// windows that waited past the grace period.
    fn refresh_locks(&mut self, now: Instant) {
        let mut changed = Vec::new();
        let mut closed = Vec::new();
        for (id, win) in &mut self.windows {
            let state = win.lock.state(now);
            if state != win.sent_lock {
                win.sent_lock = state;
                win.info.lock = state;
                changed.push((*id, state));
            }
            if win.gone && state == LockState::Closed && !self.stopped {
                closed.push(*id);
            }
        }
        for (window, state) in changed {
            self.broadcast(sees_windows, &HostMessage::LockChanged { window, state });
        }
        for window in closed {
            self.remove_window(window);
        }
    }

    /// Starts an encode for every idle stream the pacer has a frame for.
    fn pump(&mut self, now: Instant) {
        for (key, stream) in &mut self.streams {
            if stream.busy || stream.failed {
                continue;
            }
            let Some(job) = stream.pacer.poll(now) else {
                continue;
            };
            stream.busy = true;
            let encoder = stream.encoder.take();
            let backend = stream.backend.clone();
            let (key, generation) = (*key, stream.generation);
            let (seq, recipients) = (job.seq, job.recipients.clone());
            self.tasks.spawn_blocking(move || {
                let work = AssertUnwindSafe(|| encode_frame(encoder, &*backend, key, job));
                let (encoder, result) = std::panic::catch_unwind(work)
                    .unwrap_or_else(|_| (None, Err(EncodeFailure::Panicked)));
                Done::Encoded {
                    key,
                    generation,
                    seq,
                    recipients,
                    encoder,
                    result,
                }
            });
        }
    }

    /// Handles finished blocking work.
    fn on_done(&mut self, done: Done) {
        match done {
            Done::Encoded {
                key,
                generation,
                seq,
                recipients,
                encoder,
                result,
            } => {
                let Some(stream) = self
                    .streams
                    .get_mut(&key)
                    .filter(|s| s.generation == generation)
                else {
                    return;
                };
                stream.busy = false;
                stream.encoder = encoder;
                match result {
                    Ok(Some(message)) => {
                        let targets: Vec<_> = recipients
                            .into_iter()
                            .filter(|r| stream.viewers.contains(r))
                            .collect();
                        let after_resize = stream.awaiting_from.is_some_and(|from| seq >= from);
                        if after_resize {
                            stream.awaiting_from = None;
                        }
                        for viewer in targets {
                            self.push(viewer, message.clone());
                        }
                        if after_resize {
                            self.check_resize_done(key.0);
                        }
                    }
                    Ok(None) => stream.pacer.skipped(seq),
                    Err(error) => {
                        tracing::warn!(%error, window = key.0.0, "a GUI frame was not encoded");
                        stream.pacer.skipped(seq);
                        // Without an encoder the next frame would fail the same way.
                        stream.failed = stream.encoder.is_none();
                    }
                }
            }
            Done::Screenshot { session, message } => {
                if let Some(s) = self.sessions.get_mut(&session) {
                    s.screenshots = s.screenshots.saturating_sub(1);
                }
                if !message.is_empty() {
                    self.push(session, message);
                }
            }
        }
    }

    /// The backend stopped: every window is Closed, and connections send what
    /// is queued and close.
    fn backend_gone(&mut self) {
        self.stopped = true;
        for window in self.windows.values_mut() {
            window.lock.service_stopped();
        }
        self.refresh_locks(now());
        self.close_all();
    }

    /// Closes every connection after what is queued for it.
    fn close_all(&mut self) {
        for session in self.sessions.values() {
            session.outbox.close(true);
        }
    }
}

/// Roles that get window descriptions, lock changes and the input holder.
fn sees_windows(role: Role) -> bool {
    matches!(role, Role::Viewer | Role::Tool)
}

/// Roles that get the cursor, moves and the clipboard.
fn is_viewer(role: Role) -> bool {
    role == Role::Viewer
}

/// Encodes a host message, logging the rare one that does not fit.
fn encode(message: &HostMessage) -> Option<Bytes> {
    let mut out = BytesMut::new();
    match message.encode(&mut out) {
        Ok(()) => Some(out.freeze()),
        Err(error) => {
            tracing::warn!(%error, "a GUI message could not be encoded");
            None
        }
    }
}

/// `size` with each side cut to [`MAX_DIMENSION`].
fn fit(size: Size) -> Size {
    Size::new(size.width.min(MAX_DIMENSION), size.height.min(MAX_DIMENSION))
}

/// Cuts `text` to at most `max` bytes on a character boundary.
fn truncate(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// The encoder to keep, if any, and the frame message or why there is none.
type Encoded = (
    Option<Box<dyn VideoEncoder>>,
    Result<Option<Bytes>, EncodeFailure>,
);

/// Encodes one frame of a stream and writes its message. Runs on the
/// blocking pool.
fn encode_frame(
    encoder: Option<Box<dyn VideoEncoder>>,
    backend: &dyn EncoderBackend,
    (window, offer): StreamKey,
    job: Encode<Frame>,
) -> Encoded {
    let mut encoder = match encoder {
        Some(encoder) => encoder,
        None => match backend.encoder() {
            Ok(encoder) => encoder,
            Err(error) => return (None, Err(error.into())),
        },
    };
    let params = EncodeParams {
        bitrate_bps: job.target.bitrate_bps,
        frame_interval: job.target.frame_interval,
        quality: job.target.quality,
        keyframe: job.keyframe,
    };
    let result = match encoder.encode(&job.frame, &params) {
        Ok(Some(encoded)) => {
            let message = HostMessage::VideoFrame(VideoFrame {
                window,
                seq: job.seq,
                codec: offer.codec,
                keyframe: encoded.keyframe,
                size: Size::new(encoded.width, encoded.height),
                timestamp_us: u64::try_from(job.frame.timestamp().as_micros()).unwrap_or(u64::MAX),
                payload: encoded.payload,
            });
            let mut out = BytesMut::new();
            message.encode(&mut out).map(|()| Some(out.freeze())).map_err(Into::into)
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error.into()),
    };
    (Some(encoder), result)
}

/// One window's picture as a raw keyframe.
fn screenshot_result(window: WindowId, frame: &Frame) -> ScreenshotResult {
    let params = EncodeParams {
        bitrate_bps: 0,
        frame_interval: std::time::Duration::ZERO,
        quality: 100,
        keyframe: true,
    };
    match RawEncoder::new().encode(frame, &params) {
        Ok(Some(encoded)) => ScreenshotResult::Image(Screenshot {
            window,
            size: Size::new(encoded.width, encoded.height),
            codec: CodecId::RAW,
            payload: encoded.payload,
        }),
        // A keyframe is never skipped.
        Ok(None) => ScreenshotResult::NoFrame,
        Err(error) => ScreenshotResult::Failed(error.to_string()),
    }
}
