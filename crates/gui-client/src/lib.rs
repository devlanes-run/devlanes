//! A headless async client of the devlanes GUI protocol.
//!
//! [`connect`] or [`connect_stream`] says hello with a role and returns a
//! [`Client`] for sending and an [`Events`] stream of what the host sends.
//! Viewers get decoded frames: each window decodes on the blocking pool, a
//! frame is acknowledged once [`Events::next`] hands it over, and a frame
//! that fails to decode is acknowledged as discarded and answered with a
//! keyframe request. While the client holds a button or key it sends
//! keep-alives, so the host does not release them. Each window's lock state
//! comes from a [`devlanes_gui_proto::ViewerLock`] fed with everything that
//! arrives. Nothing here draws or reads a local input device; the agent
//! commands and the viewer build that on top.

mod driver;
mod error;
mod events;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use devlanes_gui_codec::{
    CodecError, DecoderBackend, Frame, Packet, RawBackend, RawDecoder, VideoDecoder,
};
use devlanes_gui_proto::{
    Buttons, ClientMessage, CodecId, Hello, HostMessage, Key, KeyCode, LockState, Point, Pointer,
    Role, SYNC_FEATURE, ScreenshotResult, SessionId, Size, Token, ViewerLock, Welcome, Wheel,
    WindowId,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub use error::ClientError;
pub use events::{DecodedFrame, Event, Events};

/// How long the host has to answer the hello.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Who the client is and what it can decode.
#[derive(Clone)]
pub struct ClientOptions {
    /// What the connection is for.
    pub role: Role,
    /// A stable id, such as the machine and process, for the host's logs and
    /// the input holder notice.
    pub client_id: String,
    /// A name for people.
    pub client_name: String,
    /// The decoders a viewer offers; empty for other roles.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// The token of a [`Role::Control`] connection.
    pub token: Option<Token>,
}

impl ClientOptions {
    /// Options for `role`. A viewer decodes `raw`; add others with
    /// [`ClientOptions::with_decoders`].
    pub fn new(role: Role, client_id: impl Into<String>, client_name: impl Into<String>) -> Self {
        let decoders: Vec<Arc<dyn DecoderBackend>> = match role {
            Role::Viewer => vec![Arc::new(RawBackend)],
            _ => Vec::new(),
        };
        Self {
            role,
            client_id: client_id.into(),
            client_name: client_name.into(),
            decoders,
            token: None,
        }
    }

    /// The same options with `decoders` instead of the default ones.
    pub fn with_decoders(mut self, decoders: Vec<Arc<dyn DecoderBackend>>) -> Self {
        self.decoders = decoders;
        self
    }

    /// The same options with the control token.
    pub fn with_token(mut self, token: Token) -> Self {
        self.token = Some(token);
        self
    }
}

impl std::fmt::Debug for ClientOptions {
    /// The role and names; decoders by offer, the token redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let decoders: Vec<_> = self.decoders.iter().map(|d| d.offer()).collect();
        f.debug_struct("ClientOptions")
            .field("role", &self.role)
            .field("client_id", &self.client_id)
            .field("client_name", &self.client_name)
            .field("decoders", &decoders)
            .field("token", &self.token)
            .finish()
    }
}

/// A screenshot: one window's picture.
#[derive(Debug, Clone)]
pub struct Image {
    /// The window it shows.
    pub window: WindowId,
    /// The picture, BGRA.
    pub frame: Frame,
}

/// Connects to a GUI host on `addr` over TCP and says hello.
pub async fn connect(
    addr: impl ToSocketAddrs,
    options: ClientOptions,
) -> Result<(Client, Events), ClientError> {
    let stream = TcpStream::connect(addr).await?;
    // Input and acknowledgements are small; Nagle would hold them back.
    stream.set_nodelay(true)?;
    connect_stream(stream, options).await
}

/// Says hello on a connected stream, such as a mirrored port or an in-memory
/// pipe, and waits up to [`ANSWER_TIMEOUT`] for the welcome.
pub async fn connect_stream<S>(
    stream: S,
    options: ClientOptions,
) -> Result<(Client, Events), ClientError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let hello = Hello {
        role: options.role,
        client_id: options.client_id.clone(),
        client_name: options.client_name.clone(),
        decoders: options.decoders.iter().map(|d| d.offer()).collect(),
        features: vec![SYNC_FEATURE.to_owned()],
        token: options.token.clone(),
    };
    let mut out = BytesMut::new();
    ClientMessage::Hello(hello).encode(&mut out)?;
    writer.write_all(&out).await?;

    let mut buf = BytesMut::with_capacity(16 * 1024);
    let answer = tokio::time::timeout(ANSWER_TIMEOUT, read_answer(&mut reader, &mut buf))
        .await
        .map_err(|_| ClientError::Timeout)??;
    let welcome = match answer {
        HostMessage::Welcome(welcome) => welcome,
        HostMessage::Refused(refusal) => {
            return Err(ClientError::Refused {
                reason: refusal.reason,
                message: refusal.message,
            });
        }
        // `decode_answer` returns nothing else.
        _ => return Err(devlanes_gui_proto::DecodeError::BadMagic.into()),
    };

    let (writer_tx, writer_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let now = driver::now();
    let windows = welcome
        .windows
        .iter()
        .map(|info| (info.id, WindowState::new(info.lock, now)))
        .collect();
    let shared = Arc::new(Shared {
        welcome,
        writer: writer_tx,
        events: events_tx,
        state: Mutex::new(State {
            windows,
            screenshots: HashMap::new(),
            syncs: HashMap::new(),
            next_request: 1,
            held: Held::default(),
        }),
        wake: Notify::new(),
        close: CancellationToken::new(),
    });
    tokio::spawn(driver::write_loop(writer, writer_rx, shared.clone()));
    tokio::spawn(driver::read_loop(
        reader,
        buf,
        shared.clone(),
        options.decoders,
    ));
    let client = Client {
        shared: shared.clone(),
        _guard: Arc::new(CloseOnDrop(shared.close.clone())),
    };
    Ok((client, Events::new(events_rx, shared)))
}

/// Reads until the host's first message is complete.
async fn read_answer<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut BytesMut,
) -> Result<HostMessage, ClientError> {
    loop {
        if let Some(answer) = HostMessage::decode_answer(buf)? {
            return Ok(answer);
        }
        if reader.read_buf(buf).await? == 0 {
            return Err(ClientError::Closed);
        }
    }
}

/// What the client tracks per window.
#[derive(Debug)]
struct WindowState {
    lock: ViewerLock,
    /// The state last reported in [`Event::Lock`] or the window description.
    shown: LockState,
    /// The next resize request number; never 0.
    next_resize: u32,
}

impl WindowState {
    /// A window the host described with `host` as its lock state.
    fn new(host: LockState, now: std::time::Instant) -> Self {
        let mut lock = ViewerLock::new(now);
        lock.host_state(host);
        Self {
            shown: lock.state(now),
            lock,
            next_resize: 1,
        }
    }
}

/// Buttons and keys this client holds down, for keep-alives.
#[derive(Debug, Default)]
struct Held {
    buttons: Buttons,
    keys: Vec<KeyCode>,
}

impl Held {
    /// Whether anything is held.
    fn any(&self) -> bool {
        !self.buttons.is_empty() || !self.keys.is_empty()
    }

    /// Follows a message about to be sent.
    fn track(&mut self, message: &ClientMessage) {
        match message {
            ClientMessage::Pointer(pointer) => self.buttons = pointer.buttons,
            ClientMessage::Key(key) if key.down => {
                if !self.keys.contains(&key.code) {
                    self.keys.push(key.code);
                }
            }
            ClientMessage::Key(key) => self.keys.retain(|code| *code != key.code),
            _ => {}
        }
    }
}

/// State shared by the handles and the connection's tasks.
#[derive(Debug)]
struct State {
    windows: HashMap<WindowId, WindowState>,
    screenshots: HashMap<u32, oneshot::Sender<ScreenshotResult>>,
    syncs: HashMap<u32, oneshot::Sender<()>>,
    next_request: u32,
    held: Held,
}

/// The connection, shared by [`Client`], [`Events`] and the tasks.
#[derive(Debug)]
struct Shared {
    welcome: Welcome,
    writer: mpsc::UnboundedSender<Bytes>,
    events: mpsc::UnboundedSender<events::Slot>,
    state: Mutex<State>,
    /// Wakes the reader to recompute its timer after a resize request.
    wake: Notify,
    close: CancellationToken,
}

impl Shared {
    /// The shared state. A panic while it was held leaves nothing half
    /// updated that matters, so a poisoned lock is used as is.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// Encodes `message` and queues it for the writer.
    fn send(&self, message: &ClientMessage) -> Result<(), ClientError> {
        let mut out = BytesMut::new();
        message.encode(&mut out)?;
        self.state().held.track(message);
        self.writer
            .send(out.freeze())
            .map_err(|_| ClientError::Closed)
    }

    /// Queues `event` for [`Events`]; nobody may be reading any more.
    fn emit(&self, event: Event) {
        let _ = self.events.send(events::Slot::Event(event));
    }

    /// Reports `window`'s lock state if it changed since it was last reported.
    fn refresh_lock(&self, window: WindowId, state: &mut WindowState, now: std::time::Instant) {
        let current = state.lock.state(now);
        if current != state.shown {
            state.shown = current;
            self.emit(Event::Lock {
                window,
                state: current,
            });
        }
    }

    /// A fresh request number for a screenshot or a sync.
    fn request_number(state: &mut State) -> u32 {
        let request = state.next_request;
        state.next_request = state.next_request.wrapping_add(1);
        request
    }
}

/// Closes the connection when the last [`Client`] clone is dropped.
#[derive(Debug)]
struct CloseOnDrop(CancellationToken);

impl Drop for CloseOnDrop {
    /// Closes the connection.
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Sends input and requests to the host. Clones share one connection, which
/// closes when the last clone is dropped or on [`Client::close`].
#[derive(Debug, Clone)]
pub struct Client {
    shared: Arc<Shared>,
    _guard: Arc<CloseOnDrop>,
}

impl Client {
    /// The host's welcome.
    pub fn welcome(&self) -> &Welcome {
        &self.shared.welcome
    }

    /// The session id the host gave this connection.
    pub fn session(&self) -> SessionId {
        self.shared.welcome.session
    }

    /// Whether the host lists `feature`.
    pub fn host_has(&self, feature: &str) -> bool {
        self.shared.welcome.features.iter().any(|f| f == feature)
    }

    /// The lock state of `window` as last reported, or `None` for a window
    /// the client does not know.
    pub fn lock_state(&self, window: WindowId) -> Option<LockState> {
        self.shared.state().windows.get(&window).map(|w| w.shown)
    }

    /// Sends pointer motion and buttons.
    pub fn pointer(&self, pointer: Pointer) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::Pointer(pointer))
    }

    /// Moves the pointer to `position` in `window`, presses `button` there
    /// and releases it.
    pub fn click(
        &self,
        window: WindowId,
        position: Point,
        button: Buttons,
    ) -> Result<(), ClientError> {
        for buttons in [Buttons::NONE, button, Buttons::NONE] {
            self.pointer(Pointer {
                window,
                position,
                buttons,
                screen_delta: None,
            })?;
        }
        Ok(())
    }

    /// The pointer left `window` with no button held.
    pub fn pointer_left(&self, window: WindowId) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::PointerLeft { window })
    }

    /// Sends a scroll.
    pub fn wheel(&self, wheel: Wheel) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::Wheel(wheel))
    }

    /// Sends a key going down or up.
    pub fn key(&self, window: WindowId, code: KeyCode, down: bool) -> Result<(), ClientError> {
        self.shared
            .send(&ClientMessage::Key(Key { window, code, down }))
    }

    /// Sends committed text.
    pub fn text(&self, window: WindowId, text: impl Into<String>) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::Text {
            window,
            text: text.into(),
        })
    }

    /// Asks for `window` at `size` and returns the request number, which the
    /// host's [`Event::ResizeApplied`] echoes. The window shows Resizing
    /// until the first frame after the change or [`devlanes_gui_proto::RESIZE_TIMEOUT`].
    pub fn resize(&self, window: WindowId, size: Size) -> Result<u32, ClientError> {
        let now = driver::now();
        let request = {
            let mut state = self.shared.state();
            let Some(win) = state.windows.get_mut(&window) else {
                return Err(ClientError::NoSuchWindow);
            };
            let request = win.next_resize;
            win.next_resize = match win.next_resize.wrapping_add(1) {
                0 => 1,
                next => next,
            };
            win.lock.resize_requested(request, now);
            self.shared.refresh_lock(window, win, now);
            request
        };
        self.shared.wake.notify_one();
        self.shared.send(&ClientMessage::Resize {
            window,
            size,
            request,
        })?;
        Ok(request)
    }

    /// Asks the app to close `window`.
    pub fn close_window(&self, window: WindowId) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::Close { window })
    }

    /// Tells the host the local window of `window` gained or lost focus.
    pub fn focus(&self, window: WindowId, focused: bool) -> Result<(), ClientError> {
        self.shared
            .send(&ClientMessage::Focus { window, focused })
    }

    /// Sends the local clipboard text.
    pub fn clipboard(&self, text: impl Into<String>) -> Result<(), ClientError> {
        self.shared
            .send(&ClientMessage::Clipboard { text: text.into() })
    }

    /// Asks for a keyframe of `window`.
    pub fn request_keyframe(&self, window: WindowId) -> Result<(), ClientError> {
        self.shared
            .send(&ClientMessage::KeyframeRequest { window })
    }

    /// Takes one window's picture, or the host's first top-level window's
    /// when `window` is `None`, without watching its stream.
    pub async fn screenshot(&self, window: Option<WindowId>) -> Result<Image, ClientError> {
        let (tx, rx) = oneshot::channel();
        let request = {
            let mut state = self.shared.state();
            let request = Shared::request_number(&mut state);
            state.screenshots.insert(request, tx);
            request
        };
        self.shared
            .send(&ClientMessage::Screenshot { request, window })?;
        let shot = match rx.await.map_err(|_| ClientError::Closed)? {
            ScreenshotResult::Image(shot) => shot,
            ScreenshotResult::NoSuchWindow => return Err(ClientError::NoSuchWindow),
            ScreenshotResult::NoFrame => return Err(ClientError::NoFrame),
            ScreenshotResult::Failed(message) => return Err(ClientError::ScreenshotFailed(message)),
        };
        if shot.codec != CodecId::RAW {
            return Err(ClientError::UnsupportedCodec(shot.codec));
        }
        let decode = move || {
            RawDecoder::new().decode(&Packet {
                payload: &shot.payload,
                width: shot.size.width,
                height: shot.size.height,
                keyframe: true,
                timestamp: Duration::ZERO,
            })
        };
        let frame = tokio::task::spawn_blocking(decode)
            .await
            .map_err(|_| ClientError::Closed)??
            .ok_or(CodecError::InvalidFrame(
                "the raw decoder returned no picture",
            ))?;
        Ok(Image {
            window: shot.window,
            frame,
        })
    }

    /// Waits until the host has applied every input this client sent before
    /// and captured a frame after it, such as between a click and typing.
    pub async fn sync(&self) -> Result<(), ClientError> {
        if !self.host_has(SYNC_FEATURE) {
            return Err(ClientError::Unsupported("sync"));
        }
        let (tx, rx) = oneshot::channel();
        let request = {
            let mut state = self.shared.state();
            let request = Shared::request_number(&mut state);
            state.syncs.insert(request, tx);
            request
        };
        self.shared.send(&ClientMessage::Sync { request })?;
        rx.await.map_err(|_| ClientError::Closed)
    }

    /// Control: tells the host the unit's process identity.
    pub fn unit_process(
        &self,
        pid: u32,
        start_time: u64,
        job: Option<String>,
    ) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::UnitProcess {
            pid,
            start_time,
            job,
        })
    }

    /// Control: the unit's process exited.
    pub fn unit_exited(&self, code: Option<i32>) -> Result<(), ClientError> {
        self.shared.send(&ClientMessage::UnitExited { code })
    }

    /// Control: the lane was frozen or thawed.
    pub fn lane_frozen(&self, frozen: bool) -> Result<(), ClientError> {
        self.shared.send(&match frozen {
            true => ClientMessage::LaneFrozen,
            false => ClientMessage::LaneThawed,
        })
    }

    /// Closes the connection for every clone.
    pub fn close(&self) {
        self.shared.close.cancel();
    }
}

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
