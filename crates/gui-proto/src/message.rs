//! The messages of both directions and their encoding.

use bytes::{Bytes, BytesMut};

use crate::error::{DecodeError, EncodeError};
use crate::types::{
    Buttons, CodecId, CodecOffer, CursorShape, Edge, KeyCode, LockState, Point, Role, SessionId,
    Size, Token, WheelUnit, WindowId, WindowInfo,
};
use crate::wire::{self, Direction, Reader, Writer};
use crate::{MAGIC, PROTOCOL_VERSION};

/// Client id and token length limit, in bytes.
pub const MAX_ID: usize = 128;
/// Client name, service name and input holder name limit, in bytes.
pub const MAX_NAME: usize = 256;
/// Feature name length limit, in bytes.
pub const MAX_FEATURE: usize = 64;
/// Features one end may list.
pub const MAX_FEATURES: usize = 64;
/// Codecs one end may list.
pub const MAX_CODECS: usize = 32;
/// Windows a welcome may list.
pub const MAX_WINDOWS: usize = 256;
/// Committed text limit per message, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Clipboard text limit, in bytes.
pub const MAX_CLIPBOARD: usize = 4 * 1024 * 1024;
/// Limit of free-form messages: refusals, failed screenshots, job names.
pub const MAX_MESSAGE: usize = 1024;
/// Largest video frame or screenshot payload: a raw 8192 x 8192 BGRA frame
/// (256 MiB) plus zstd's worst-case growth on data that does not compress.
pub const MAX_FRAME_PAYLOAD: usize = 257 * 1024 * 1024;

/// Body limit of a hello. Frozen together with the frame header and the
/// preamble: a hello of any later version must fit, so this version still
/// reads its version and reports a mismatch instead of [`DecodeError::TooLarge`].
pub const MAX_HELLO: u32 = 64 * 1024;
/// Body limit of a refusal, frozen for the same reason as [`MAX_HELLO`].
pub const MAX_REFUSAL: u32 = MAX_MESSAGE as u32 + SLACK;

/// The feature that adds [`ClientMessage::Sync`] and [`HostMessage::Synced`].
pub const SYNC_FEATURE: &str = "sync";

/// Room left in each limit for fields later versions append.
const SLACK: u32 = 1024;
/// Limit of messages that carry only a few numbers.
const SMALL: u32 = 64 + SLACK;

/// Type bytes of messages to the host. Values are never reused.
mod to_host {
    pub const HELLO: u8 = 0x01;
    pub const FRAME_ACK: u8 = 0x02;
    pub const KEYFRAME_REQUEST: u8 = 0x03;
    pub const POINTER: u8 = 0x04;
    pub const WHEEL: u8 = 0x05;
    pub const KEY: u8 = 0x06;
    pub const TEXT: u8 = 0x07;
    pub const RESIZE: u8 = 0x08;
    pub const CLOSE: u8 = 0x09;
    pub const FOCUS: u8 = 0x0a;
    pub const CLIPBOARD: u8 = 0x0b;
    pub const SCREENSHOT: u8 = 0x0c;
    pub const KEEP_ALIVE: u8 = 0x0d;
    pub const POINTER_LEFT: u8 = 0x0e;
    pub const SYNC: u8 = 0x0f;
    pub const UNIT_PROCESS: u8 = 0x20;
    pub const UNIT_EXITED: u8 = 0x21;
    pub const LANE_FROZEN: u8 = 0x22;
    pub const LANE_THAWED: u8 = 0x23;
}

/// Type bytes of messages to clients. Values are never reused.
mod to_client {
    pub const WELCOME: u8 = 0x81;
    pub const REFUSED: u8 = 0x82;
    pub const WINDOW_ADDED: u8 = 0x83;
    pub const WINDOW_CHANGED: u8 = 0x84;
    pub const WINDOW_REMOVED: u8 = 0x85;
    pub const VIDEO_FRAME: u8 = 0x86;
    pub const CURSOR: u8 = 0x87;
    pub const MOVE_RESIZE: u8 = 0x88;
    pub const LOCK_CHANGED: u8 = 0x89;
    pub const CLIPBOARD: u8 = 0x8a;
    pub const INPUT_HOLDER: u8 = 0x8b;
    pub const SCREENSHOT: u8 = 0x8c;
    pub const HEARTBEAT: u8 = 0x8d;
    pub const RESIZE_APPLIED: u8 = 0x8e;
    pub const SYNCED: u8 = 0x8f;
    pub const ACTIVITY: u8 = 0xa0;
}

/// Body limit of each message type to the host, `None` for unknown types.
fn host_limit(kind: u8) -> Option<u32> {
    use to_host::*;
    Some(match kind {
        HELLO => MAX_HELLO,
        TEXT => MAX_TEXT as u32 + SLACK,
        CLIPBOARD => MAX_CLIPBOARD as u32 + SLACK,
        UNIT_PROCESS => MAX_MESSAGE as u32 + SLACK,
        FRAME_ACK | KEYFRAME_REQUEST | POINTER | WHEEL | KEY | RESIZE | CLOSE | FOCUS
        | SCREENSHOT | KEEP_ALIVE | POINTER_LEFT | SYNC | UNIT_EXITED | LANE_FROZEN
        | LANE_THAWED => SMALL,
        _ => return None,
    })
}

/// Body limit of each message type to clients, `None` for unknown types.
fn client_limit(kind: u8) -> Option<u32> {
    use to_client::*;
    Some(match kind {
        WELCOME => 1024 * 1024,
        WINDOW_ADDED | WINDOW_CHANGED => 4 * 1024,
        REFUSED => MAX_REFUSAL,
        VIDEO_FRAME | SCREENSHOT => MAX_FRAME_PAYLOAD as u32 + SLACK,
        CURSOR => 256 * 256 * 4 + SLACK,
        CLIPBOARD => MAX_CLIPBOARD as u32 + SLACK,
        INPUT_HOLDER => (MAX_ID + MAX_NAME) as u32 + SLACK,
        WINDOW_REMOVED | MOVE_RESIZE | LOCK_CHANGED | HEARTBEAT | RESIZE_APPLIED | SYNCED
        | ACTIVITY => SMALL,
        _ => return None,
    })
}

/// Writes the magic bytes and the protocol version that start every hello,
/// welcome and refusal. Their layout never changes, so peers of any version
/// can tell a version mismatch apart from garbage.
fn write_preamble(w: &mut Writer<'_>) {
    for byte in MAGIC {
        w.u8(byte);
    }
    w.u16(PROTOCOL_VERSION);
}

/// Checks the magic bytes and the version before anything else is read, since
/// the rest of the body may have another layout in another version.
fn read_preamble(r: &mut Reader) -> Result<(), DecodeError> {
    for byte in MAGIC {
        if r.u8("magic").map_err(|_| DecodeError::BadMagic)? != byte {
            return Err(DecodeError::BadMagic);
        }
    }
    let theirs = r.u16("protocol version")?;
    if theirs != PROTOCOL_VERSION {
        return Err(DecodeError::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs,
        });
    }
    Ok(())
}

/// Reads a list of codec offers.
fn read_offers(r: &mut Reader, field: &'static str) -> Result<Vec<CodecOffer>, DecodeError> {
    let count = r.count(field, MAX_CODECS)?;
    (0..count).map(|_| CodecOffer::read(r)).collect()
}

/// Writes a list of codec offers.
fn write_offers(
    w: &mut Writer<'_>,
    field: &'static str,
    offers: &[CodecOffer],
) -> Result<(), EncodeError> {
    w.count(field, offers.len(), MAX_CODECS)?;
    offers.iter().try_for_each(|offer| offer.write(w))
}

/// Reads a list of feature names.
fn read_features(r: &mut Reader) -> Result<Vec<String>, DecodeError> {
    let count = r.count("features", MAX_FEATURES)?;
    (0..count)
        .map(|_| r.string("feature", MAX_FEATURE))
        .collect()
}

/// Writes a list of feature names.
fn write_features(w: &mut Writer<'_>, features: &[String]) -> Result<(), EncodeError> {
    w.count("features", features.len(), MAX_FEATURES)?;
    features
        .iter()
        .try_for_each(|feature| w.string("feature", feature, MAX_FEATURE))
}

/// Reads a presence byte and, when present, a string of at most `max` bytes.
fn read_opt_string(
    r: &mut Reader,
    field: &'static str,
    max: usize,
) -> Result<Option<String>, DecodeError> {
    Ok(match r.bool(field)? {
        true => Some(r.string(field, max)?),
        false => None,
    })
}

/// Writes a presence byte and, when present, a string.
fn write_opt_string(
    w: &mut Writer<'_>,
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<(), EncodeError> {
    w.bool(value.is_some());
    match value {
        Some(value) => w.string(field, value, max),
        None => Ok(()),
    }
}

/// Reads a window id.
fn read_window(r: &mut Reader) -> Result<WindowId, DecodeError> {
    Ok(WindowId(r.u32("window")?))
}

/// The first message of every connection, from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// What the connection is for.
    pub role: Role,
    /// A stable id of the client, such as its machine and process, for logs
    /// and the input holder notice. Two clients may send the same one.
    pub client_id: String,
    /// A name for people, such as the user and machine.
    pub client_name: String,
    /// The decoders the client has; empty for tools and the daemon. The host
    /// picks one codec per viewer from them, and viewers that got the same
    /// codec for a window share its encoder and its sequence numbers.
    pub decoders: Vec<CodecOffer>,
    /// Additive features the client implements. A side uses a feature only
    /// when both list it.
    pub features: Vec<String>,
    /// The token the daemon passed to the host, for [`Role::Control`].
    pub token: Option<Token>,
}

/// The host's answer to an accepted hello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// The id the host gave this connection, unique per host process; the
    /// input holder notice names it.
    pub session: SessionId,
    /// The service the host belongs to, such as `acme:desktop`.
    pub service: String,
    /// The encoders the host has.
    pub encoders: Vec<CodecOffer>,
    /// Whether the host's display is the machine's real desktop (a Windows
    /// host), where the windows are already on screen locally.
    pub real_desktop: bool,
    /// The windows open now.
    pub windows: Vec<WindowInfo>,
    /// Additive features the host implements.
    pub features: Vec<String>,
}

/// Why the host refused a hello. Unknown reasons decode as [`RefusalReason::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefusalReason {
    /// A reason this build does not know; the message explains it.
    Other,
    /// The client speaks another protocol version. A client reading this
    /// refusal gets [`DecodeError::VersionMismatch`] instead, since the
    /// refusal carries the host's version.
    VersionMismatch,
    /// A control hello without the right token.
    BadToken,
    /// A role this host does not serve, such as [`Role::Input`].
    RoleNotSupported,
}

/// The host's answer to a refused hello, after which it closes the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The reason.
    pub reason: RefusalReason,
    /// Text for people, at most [`MAX_MESSAGE`] bytes.
    pub message: String,
}

/// Pointer position and buttons in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pointer {
    /// The window under the pointer, or holding it while a button is down.
    pub window: WindowId,
    /// The position in window pixels.
    pub position: Point,
    /// The buttons held now; a change from the previous message is a press or release.
    pub buttons: Buttons,
    /// While a button is held, the movement on the viewer's screen since the
    /// previous message. A host whose window moves under the pointer uses it
    /// instead of positions inside the window, which would feed back.
    pub screen_delta: Option<Point>,
}

/// A wheel or touchpad scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    /// The window under the pointer.
    pub window: WindowId,
    /// The pointer position in window pixels.
    pub position: Point,
    /// Horizontal delta, positive to the right. Finite.
    pub delta_x: f32,
    /// Vertical delta, positive down. Finite.
    pub delta_y: f32,
    /// The unit of both deltas.
    pub unit: WheelUnit,
}

/// A physical key going down or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// The focused window.
    pub window: WindowId,
    /// The key.
    pub code: KeyCode,
    /// Down or up.
    pub down: bool,
}

/// Messages from a client (viewer, tool or the daemon) to the host.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    /// The first message of every connection.
    Hello(Hello),
    /// The viewer is finished with every frame of `window` up to `seq`: it
    /// decoded it, or discarded it after a decode error. It acknowledges
    /// discarded frames too, so the host's pacing never waits on them.
    FrameAck {
        /// The window.
        window: WindowId,
        /// The newest frame finished with; earlier ones count as finished too.
        seq: u64,
    },
    /// The viewer needs a keyframe of `window`, such as after a decode error.
    /// The host stops waiting for acknowledgements of frames it sent before,
    /// but honors a new request only once the keyframe answering the previous
    /// one is acknowledged.
    KeyframeRequest {
        /// The window.
        window: WindowId,
    },
    /// Pointer movement and buttons.
    Pointer(Pointer),
    /// The pointer left the viewer's window with no button held, so the app
    /// clears its hover state.
    PointerLeft {
        /// The window.
        window: WindowId,
    },
    /// A scroll.
    Wheel(Wheel),
    /// A key down or up.
    Key(Key),
    /// Text a client typed, at most [`MAX_TEXT`] bytes. The host inserts it
    /// exactly as given, as if no modifier or lock were held, whatever keys
    /// the client holds down (Shift for "A", Control and Alt for AltGr on
    /// Windows); a viewer does not forward Caps Lock or Num Lock.
    Text {
        /// The focused window.
        window: WindowId,
        /// The text.
        text: String,
    },
    /// The final size the viewer wants for a window, sent once the local
    /// resize stops.
    Resize {
        /// The window.
        window: WindowId,
        /// The size wanted; the app may pick another.
        size: Size,
        /// Chosen by the viewer, increasing per window, and echoed in
        /// [`HostMessage::ResizeApplied`], so an answer to an older request
        /// does not end the lock of a newer one. Never 0: a viewer whose
        /// counter wraps skips 0, which answers to other viewers carry.
        request: u32,
    },
    /// Ask the app to close a window.
    Close {
        /// The window.
        window: WindowId,
    },
    /// The viewer's window gained or lost focus.
    Focus {
        /// The window.
        window: WindowId,
        /// Whether it has focus now.
        focused: bool,
    },
    /// The viewer's clipboard text changed, at most [`MAX_CLIPBOARD`] bytes.
    Clipboard {
        /// The text.
        text: String,
    },
    /// Ask for one frame of a window, answered by [`HostMessage::Screenshot`].
    Screenshot {
        /// Echoed in the answer.
        request: u32,
        /// The window, or `None` for the host's first top-level window.
        window: Option<WindowId>,
    },
    /// Nothing but proof that the client is there. A client holding a button
    /// or key sends it, or pointer messages, at least every
    /// [`crate::HEARTBEAT_INTERVAL`]; a holder silent for
    /// [`crate::HELD_INPUT_TIMEOUT`] has its keys and buttons released.
    KeepAlive,
    /// Asks for [`HostMessage::Synced`] with the same `request` once the host
    /// has applied every input received before this message and captured a
    /// frame after that. Sent only when both ends list [`SYNC_FEATURE`]; tools
    /// use it to wait for a click to land before they type.
    Sync {
        /// Echoed in the answer.
        request: u32,
    },
    /// Control: the identity of the unit's process. A PID alone can be
    /// reused, so the start time comes with it.
    UnitProcess {
        /// The process id.
        pid: u32,
        /// The process start time as `devlanes-sys` reports it
        /// (`ProcessIdentity::start_time`): clock ticks since boot on Linux,
        /// microseconds since the epoch on macOS, the creation `FILETIME` on
        /// Windows. Compared for equality only.
        start_time: u64,
        /// The name of the unit's job object on Windows, when the runner has one.
        job: Option<String>,
    },
    /// Control: the unit's process exited.
    UnitExited {
        /// The exit code, when there is one.
        code: Option<i32>,
    },
    /// Control: the lane was frozen for inactivity.
    LaneFrozen,
    /// Control: the lane was thawed.
    LaneThawed,
}

impl ClientMessage {
    /// Appends this message as one frame to `out`. On error nothing is appended.
    pub fn encode(&self, out: &mut BytesMut) -> Result<(), EncodeError> {
        let kind = self.kind();
        let max = host_limit(kind).unwrap_or(0);
        wire::write_frame(out, kind, max, |w| {
            match self {
                Self::Hello(hello) => {
                    write_preamble(w);
                    w.u8(hello.role.to_u8());
                    w.string("client id", &hello.client_id, MAX_ID)?;
                    w.string("client name", &hello.client_name, MAX_NAME)?;
                    write_offers(w, "decoders", &hello.decoders)?;
                    write_features(w, &hello.features)?;
                    let token = hello.token.as_ref().map(Token::expose);
                    write_opt_string(w, "token", token, MAX_ID)?;
                }
                Self::FrameAck { window, seq } => {
                    w.u32(window.0);
                    w.u64(*seq);
                }
                Self::KeyframeRequest { window }
                | Self::Close { window }
                | Self::PointerLeft { window } => w.u32(window.0),
                Self::Pointer(pointer) => {
                    w.u32(pointer.window.0);
                    pointer.position.write(w);
                    w.u8(pointer.buttons.0);
                    Point::write_opt(pointer.screen_delta, w);
                }
                Self::Wheel(wheel) => {
                    w.u32(wheel.window.0);
                    wheel.position.write(w);
                    w.f32("wheel delta", wheel.delta_x)?;
                    w.f32("wheel delta", wheel.delta_y)?;
                    w.u8(match wheel.unit {
                        WheelUnit::Pixels => 0,
                        WheelUnit::Lines => 1,
                    });
                }
                Self::Key(key) => {
                    w.u32(key.window.0);
                    w.u32(key.code.0);
                    w.bool(key.down);
                }
                Self::Text { window, text } => {
                    w.u32(window.0);
                    w.string("text", text, MAX_TEXT)?;
                }
                Self::Resize {
                    window,
                    size,
                    request,
                } => {
                    w.u32(window.0);
                    size.write(w, "size")?;
                    w.u32(*request);
                }
                Self::Focus { window, focused } => {
                    w.u32(window.0);
                    w.bool(*focused);
                }
                Self::Clipboard { text } => w.string("clipboard", text, MAX_CLIPBOARD)?,
                Self::Screenshot { request, window } => {
                    w.u32(*request);
                    w.bool(window.is_some());
                    if let Some(window) = window {
                        w.u32(window.0);
                    }
                }
                Self::Sync { request } => w.u32(*request),
                Self::UnitProcess {
                    pid,
                    start_time,
                    job,
                } => {
                    w.u32(*pid);
                    w.u64(*start_time);
                    write_opt_string(w, "job", job.as_deref(), MAX_MESSAGE)?;
                }
                Self::UnitExited { code } => {
                    w.bool(code.is_some());
                    if let Some(code) = code {
                        w.i32(*code);
                    }
                }
                Self::KeepAlive | Self::LaneFrozen | Self::LaneThawed => {}
            }
            Ok(())
        })
    }

    /// Whether a connection that said hello with `role` may send this message.
    /// The host checks every message after the hello and closes connections
    /// that break this. A hello is never allowed after the first one.
    pub fn allowed_for(&self, role: Role) -> bool {
        let input = matches!(
            self,
            Self::Pointer(_)
                | Self::PointerLeft { .. }
                | Self::Wheel(_)
                | Self::Key(_)
                | Self::Text { .. }
                | Self::Resize { .. }
                | Self::Close { .. }
                | Self::Focus { .. }
                | Self::Clipboard { .. }
                | Self::KeepAlive
                | Self::Sync { .. }
        );
        let video = matches!(self, Self::FrameAck { .. } | Self::KeyframeRequest { .. });
        let control = matches!(
            self,
            Self::UnitProcess { .. }
                | Self::UnitExited { .. }
                | Self::LaneFrozen
                | Self::LaneThawed
        );
        let screenshot = matches!(self, Self::Screenshot { .. });
        match role {
            Role::Viewer => input || video || screenshot,
            Role::Tool => input || screenshot,
            Role::Control => control,
            // What the second connection of a viewer session will carry.
            Role::Input => input || video,
            Role::Unknown(_) => false,
        }
    }

    /// The type byte of this message.
    fn kind(&self) -> u8 {
        use to_host::*;
        match self {
            Self::Hello(_) => HELLO,
            Self::FrameAck { .. } => FRAME_ACK,
            Self::KeyframeRequest { .. } => KEYFRAME_REQUEST,
            Self::Pointer(_) => POINTER,
            Self::PointerLeft { .. } => POINTER_LEFT,
            Self::Wheel(_) => WHEEL,
            Self::Key(_) => KEY,
            Self::Text { .. } => TEXT,
            Self::Resize { .. } => RESIZE,
            Self::Close { .. } => CLOSE,
            Self::Focus { .. } => FOCUS,
            Self::Clipboard { .. } => CLIPBOARD,
            Self::Screenshot { .. } => SCREENSHOT,
            Self::KeepAlive => KEEP_ALIVE,
            Self::Sync { .. } => SYNC,
            Self::UnitProcess { .. } => UNIT_PROCESS,
            Self::UnitExited { .. } => UNIT_EXITED,
            Self::LaneFrozen => LANE_FROZEN,
            Self::LaneThawed => LANE_THAWED,
        }
    }

    /// Takes the next complete message out of `buf`, the bytes read so far from
    /// a client.
    ///
    /// Returns `Ok(None)` until a whole frame has arrived; call it again after
    /// appending more bytes. Frames of unknown types are skipped. Each call
    /// costs time linear in the bytes it consumes, and `buf` never needs to
    /// hold more than one frame within its type's limit.
    ///
    /// After any error, close the connection. An error in a frame header
    /// leaves the header in `buf`, so calling again returns the same error.
    pub fn decode(buf: &mut BytesMut) -> Result<Option<Self>, DecodeError> {
        let Some((kind, body)) = wire::next_frame(buf, Direction::ToHost, host_limit)? else {
            return Ok(None);
        };
        Self::decode_body(Reader::new(kind, body)).map(Some)
    }

    /// Takes the first message of a connection, which must be a hello.
    ///
    /// Any other first byte is [`DecodeError::BadMagic`] as soon as it arrives,
    /// so a peer speaking HTTP or another protocol on the port fails cleanly
    /// instead of being skipped as an unknown type. Use [`ClientMessage::decode`]
    /// for the messages after it.
    pub fn decode_hello(buf: &mut BytesMut) -> Result<Option<Hello>, DecodeError> {
        if buf.first().is_some_and(|kind| *kind != to_host::HELLO) {
            return Err(DecodeError::BadMagic);
        }
        match Self::decode(buf)? {
            Some(Self::Hello(hello)) => Ok(Some(hello)),
            Some(_) => Err(DecodeError::BadMagic),
            None => Ok(None),
        }
    }

    /// Reads the body of a message whose type is known.
    fn decode_body(mut r: Reader) -> Result<Self, DecodeError> {
        use to_host::*;
        let r = &mut r;
        Ok(match r.kind() {
            HELLO => {
                read_preamble(r)?;
                Self::Hello(Hello {
                    role: Role::from_u8(r.u8("role")?),
                    client_id: r.string("client id", MAX_ID)?,
                    client_name: r.string("client name", MAX_NAME)?,
                    decoders: read_offers(r, "decoders")?,
                    features: read_features(r)?,
                    token: read_opt_string(r, "token", MAX_ID)?.map(Token::new),
                })
            }
            FRAME_ACK => Self::FrameAck {
                window: read_window(r)?,
                seq: r.u64("sequence number")?,
            },
            KEYFRAME_REQUEST => Self::KeyframeRequest {
                window: read_window(r)?,
            },
            POINTER => Self::Pointer(Pointer {
                window: read_window(r)?,
                position: Point::read(r, "position")?,
                buttons: Buttons(r.u8("buttons")?),
                screen_delta: Point::read_opt(r, "screen delta")?,
            }),
            POINTER_LEFT => Self::PointerLeft {
                window: read_window(r)?,
            },
            WHEEL => Self::Wheel(Wheel {
                window: read_window(r)?,
                position: Point::read(r, "position")?,
                delta_x: r.f32("wheel delta")?,
                delta_y: r.f32("wheel delta")?,
                unit: match r.u8("wheel unit")? {
                    0 => WheelUnit::Pixels,
                    1 => WheelUnit::Lines,
                    other => return Err(r.invalid("wheel unit", other.into())),
                },
            }),
            KEY => Self::Key(Key {
                window: read_window(r)?,
                code: KeyCode(r.u32("key code")?),
                down: r.bool("key down")?,
            }),
            TEXT => Self::Text {
                window: read_window(r)?,
                text: r.string("text", MAX_TEXT)?,
            },
            RESIZE => Self::Resize {
                window: read_window(r)?,
                size: Size::read(r, "size")?,
                request: r.u32("request")?,
            },
            CLOSE => Self::Close {
                window: read_window(r)?,
            },
            FOCUS => Self::Focus {
                window: read_window(r)?,
                focused: r.bool("focused")?,
            },
            CLIPBOARD => Self::Clipboard {
                text: r.string("clipboard", MAX_CLIPBOARD)?,
            },
            SCREENSHOT => Self::Screenshot {
                request: r.u32("request")?,
                window: match r.bool("window")? {
                    true => Some(read_window(r)?),
                    false => None,
                },
            },
            KEEP_ALIVE => Self::KeepAlive,
            SYNC => Self::Sync {
                request: r.u32("request")?,
            },
            UNIT_PROCESS => Self::UnitProcess {
                pid: r.u32("pid")?,
                start_time: r.u64("start time")?,
                job: read_opt_string(r, "job", MAX_MESSAGE)?,
            },
            UNIT_EXITED => Self::UnitExited {
                code: match r.bool("exit code")? {
                    true => Some(r.i32("exit code")?),
                    false => None,
                },
            },
            LANE_FROZEN => Self::LaneFrozen,
            LANE_THAWED => Self::LaneThawed,
            // Only reached if a type gets a limit without a decoder.
            other => return Err(r.invalid("message type", other.into())),
        })
    }
}

/// One encoded frame of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// The window.
    pub window: WindowId,
    /// Increasing per window and codec, since viewers that use another codec
    /// get another stream; acknowledged with [`ClientMessage::FrameAck`].
    pub seq: u64,
    /// The codec of the payload.
    pub codec: CodecId,
    /// Whether the frame decodes without the ones before it.
    pub keyframe: bool,
    /// The picture's size, 1 to [`crate::MAX_DIMENSION`] on each side. A frame at
    /// a new size is a keyframe.
    pub size: Size,
    /// When the host captured it, in microseconds on the host's monotonic clock.
    pub timestamp_us: u64,
    /// The encoded picture, at most [`MAX_FRAME_PAYLOAD`] bytes.
    pub payload: Bytes,
}

/// The cursor the app shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// The window it applies to.
    pub window: WindowId,
    /// Its shape.
    pub shape: CursorShape,
    /// Where the host's real pointer is in the window, on a host whose pointer
    /// is shared with its own user (Windows); `None` elsewhere.
    pub position: Option<Point>,
}

/// The app started an interactive move or resize, such as a press on its own
/// title bar. A borderless viewer window starts a local drag in answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MoveResize {
    /// The window.
    pub window: WindowId,
    /// `None` for a move, the edge or corner for a resize.
    pub edge: Option<Edge>,
    /// Where the pointer grabbed the window, in window pixels.
    pub grab: Point,
}

/// The client that holds input on the display.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Holder {
    /// The holder's session; a viewer holds input when it equals its own
    /// [`Welcome::session`].
    pub session: SessionId,
    /// The holder's [`Hello::client_id`].
    pub client_id: String,
    /// The holder's [`Hello::client_name`].
    pub name: String,
}

/// One window's picture, for a screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    /// The window.
    pub window: WindowId,
    /// The picture's size.
    pub size: Size,
    /// The payload's codec, normally [`CodecId::RAW`]; the caller turns it into
    /// PNG or whatever it needs.
    pub codec: CodecId,
    /// A keyframe in that codec.
    pub payload: Bytes,
}

/// The outcome of a screenshot request. Unknown outcomes decode as
/// [`ScreenshotResult::Failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenshotResult {
    /// The picture.
    Image(Screenshot),
    /// No such window, or no window at all when none was named.
    NoSuchWindow,
    /// The window has not drawn a frame yet.
    NoFrame,
    /// Capture or encoding failed, with a reason for people.
    Failed(String),
}

/// Messages from the host to its clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMessage {
    /// The answer to an accepted hello.
    Welcome(Welcome),
    /// The answer to a refused hello.
    Refused(Refusal),
    /// A window appeared.
    WindowAdded(WindowInfo),
    /// A window's description changed; the whole new description.
    WindowChanged(WindowInfo),
    /// A window closed.
    WindowRemoved {
        /// The window.
        window: WindowId,
    },
    /// An encoded frame.
    VideoFrame(VideoFrame),
    /// The cursor changed.
    Cursor(Cursor),
    /// The app started a move or resize.
    MoveResizeStarted(MoveResize),
    /// A window's lock state changed. The host sends the end of Resizing only
    /// once the first frame captured after the change is encoded and queued to
    /// every viewer of the window, so no viewer unlocks before that frame.
    LockChanged {
        /// The window.
        window: WindowId,
        /// Its new state.
        state: LockState,
    },
    /// The app's clipboard text changed, at most [`MAX_CLIPBOARD`] bytes.
    Clipboard {
        /// The text.
        text: String,
    },
    /// Who holds input now; `None` when nobody does. Input from others is dropped.
    InputHolder(Option<Holder>),
    /// The answer to [`ClientMessage::Screenshot`].
    Screenshot {
        /// The request it answers.
        request: u32,
        /// The outcome.
        result: ScreenshotResult,
    },
    /// Sent when nothing else was sent for [`crate::HEARTBEAT_INTERVAL`], so a
    /// viewer can tell a silent link from a static window.
    Heartbeat,
    /// The host applied a size change to a window. Frames of this viewer's
    /// stream from `from_seq` on were captured after it, whatever their size;
    /// earlier ones may still show the old size. Sent to every viewer of the
    /// window, each with the sequence number of its own stream.
    ResizeApplied {
        /// The window.
        window: WindowId,
        /// The [`ClientMessage::Resize`] request it answers, for the viewer
        /// that sent it; 0 for other viewers and for a change the app made.
        /// Viewers number their requests from 1. Every `Resize` gets exactly
        /// one answer; a host that merges several requests into one change
        /// answers with the newest one's number.
        request: u32,
        /// The first sequence number captured after the change.
        from_seq: u64,
    },
    /// The answer to [`ClientMessage::Sync`].
    Synced {
        /// The request it answers.
        request: u32,
    },
    /// To the control connection: a viewer sent input, which counts as lane
    /// activity. The host sends it at most once per [`crate::HEARTBEAT_INTERVAL`].
    Activity,
}

impl HostMessage {
    /// Appends this message as one frame to `out`. On error nothing is appended.
    pub fn encode(&self, out: &mut BytesMut) -> Result<(), EncodeError> {
        let kind = self.kind();
        let max = client_limit(kind).unwrap_or(0);
        wire::write_frame(out, kind, max, |w| {
            match self {
                Self::Welcome(welcome) => {
                    write_preamble(w);
                    w.u64(welcome.session.0);
                    w.string("service", &welcome.service, MAX_NAME)?;
                    write_offers(w, "encoders", &welcome.encoders)?;
                    w.bool(welcome.real_desktop);
                    w.count("windows", welcome.windows.len(), MAX_WINDOWS)?;
                    welcome
                        .windows
                        .iter()
                        .try_for_each(|window| window.write(w))?;
                    write_features(w, &welcome.features)?;
                }
                Self::Refused(refusal) => {
                    write_preamble(w);
                    w.u8(match refusal.reason {
                        RefusalReason::Other => 0,
                        RefusalReason::VersionMismatch => 1,
                        RefusalReason::BadToken => 2,
                        RefusalReason::RoleNotSupported => 3,
                    });
                    w.string("message", &refusal.message, MAX_MESSAGE)?;
                }
                Self::WindowAdded(window) | Self::WindowChanged(window) => window.write(w)?,
                Self::WindowRemoved { window } => w.u32(window.0),
                Self::VideoFrame(frame) => {
                    if !frame.size.is_nonzero() {
                        return Err(EncodeError::InvalidValue {
                            field: "frame size",
                        });
                    }
                    w.u32(frame.window.0);
                    w.u64(frame.seq);
                    w.u8(frame.codec.0);
                    w.bool(frame.keyframe);
                    frame.size.write(w, "frame size")?;
                    w.u64(frame.timestamp_us);
                    w.bytes("payload", &frame.payload, MAX_FRAME_PAYLOAD)?;
                }
                Self::Cursor(cursor) => {
                    w.u32(cursor.window.0);
                    cursor.shape.write(w)?;
                    Point::write_opt(cursor.position, w);
                }
                Self::MoveResizeStarted(started) => {
                    w.u32(started.window.0);
                    Edge::write_opt(started.edge, w);
                    started.grab.write(w);
                }
                Self::LockChanged { window, state } => {
                    w.u32(window.0);
                    w.u8(state.to_u8());
                }
                Self::Clipboard { text } => w.string("clipboard", text, MAX_CLIPBOARD)?,
                Self::InputHolder(holder) => {
                    w.bool(holder.is_some());
                    if let Some(holder) = holder {
                        w.u64(holder.session.0);
                        w.string("holder id", &holder.client_id, MAX_ID)?;
                        w.string("holder name", &holder.name, MAX_NAME)?;
                    }
                }
                Self::Screenshot { request, result } => {
                    w.u32(*request);
                    match result {
                        ScreenshotResult::Image(shot) => {
                            if !shot.size.is_nonzero() {
                                return Err(EncodeError::InvalidValue {
                                    field: "screenshot size",
                                });
                            }
                            w.u8(0);
                            w.u32(shot.window.0);
                            shot.size.write(w, "screenshot size")?;
                            w.u8(shot.codec.0);
                            w.bytes("payload", &shot.payload, MAX_FRAME_PAYLOAD)?;
                        }
                        ScreenshotResult::NoSuchWindow => w.u8(1),
                        ScreenshotResult::NoFrame => w.u8(2),
                        ScreenshotResult::Failed(message) => {
                            w.u8(3);
                            w.string("message", message, MAX_MESSAGE)?;
                        }
                    }
                }
                Self::ResizeApplied {
                    window,
                    request,
                    from_seq,
                } => {
                    w.u32(window.0);
                    w.u32(*request);
                    w.u64(*from_seq);
                }
                Self::Synced { request } => w.u32(*request),
                Self::Heartbeat | Self::Activity => {}
            }
            Ok(())
        })
    }

    /// The type byte of this message.
    fn kind(&self) -> u8 {
        use to_client::*;
        match self {
            Self::Welcome(_) => WELCOME,
            Self::Refused(_) => REFUSED,
            Self::WindowAdded(_) => WINDOW_ADDED,
            Self::WindowChanged(_) => WINDOW_CHANGED,
            Self::WindowRemoved { .. } => WINDOW_REMOVED,
            Self::VideoFrame(_) => VIDEO_FRAME,
            Self::Cursor(_) => CURSOR,
            Self::MoveResizeStarted(_) => MOVE_RESIZE,
            Self::LockChanged { .. } => LOCK_CHANGED,
            Self::Clipboard { .. } => CLIPBOARD,
            Self::InputHolder(_) => INPUT_HOLDER,
            Self::Screenshot { .. } => SCREENSHOT,
            Self::Heartbeat => HEARTBEAT,
            Self::ResizeApplied { .. } => RESIZE_APPLIED,
            Self::Synced { .. } => SYNCED,
            Self::Activity => ACTIVITY,
        }
    }

    /// Takes the next complete message out of `buf`, the bytes read so far from
    /// the host. Behaves as [`ClientMessage::decode`] does.
    pub fn decode(buf: &mut BytesMut) -> Result<Option<Self>, DecodeError> {
        let Some((kind, body)) = wire::next_frame(buf, Direction::ToClient, client_limit)? else {
            return Ok(None);
        };
        Self::decode_body(Reader::new(kind, body)).map(Some)
    }

    /// Takes the host's answer to the hello, which must be the first message:
    /// a welcome or a refusal. Anything else is [`DecodeError::BadMagic`] as
    /// soon as its first byte arrives, as in [`ClientMessage::decode_hello`].
    pub fn decode_answer(buf: &mut BytesMut) -> Result<Option<Self>, DecodeError> {
        let answer = [to_client::WELCOME, to_client::REFUSED];
        if buf.first().is_some_and(|kind| !answer.contains(kind)) {
            return Err(DecodeError::BadMagic);
        }
        Self::decode(buf)
    }

    /// Reads the body of a message whose type is known.
    fn decode_body(mut r: Reader) -> Result<Self, DecodeError> {
        use to_client::*;
        let r = &mut r;
        Ok(match r.kind() {
            WELCOME => {
                read_preamble(r)?;
                let session = SessionId(r.u64("session")?);
                let service = r.string("service", MAX_NAME)?;
                let encoders = read_offers(r, "encoders")?;
                let real_desktop = r.bool("real desktop")?;
                let count = r.count("windows", MAX_WINDOWS)?;
                let windows = (0..count)
                    .map(|_| WindowInfo::read(r))
                    .collect::<Result<_, _>>()?;
                Self::Welcome(Welcome {
                    session,
                    service,
                    encoders,
                    real_desktop,
                    windows,
                    features: read_features(r)?,
                })
            }
            REFUSED => {
                read_preamble(r)?;
                let reason = match r.u8("reason")? {
                    1 => RefusalReason::VersionMismatch,
                    2 => RefusalReason::BadToken,
                    3 => RefusalReason::RoleNotSupported,
                    _ => RefusalReason::Other,
                };
                Self::Refused(Refusal {
                    reason,
                    message: r.string("message", MAX_MESSAGE)?,
                })
            }
            WINDOW_ADDED => Self::WindowAdded(WindowInfo::read(r)?),
            WINDOW_CHANGED => Self::WindowChanged(WindowInfo::read(r)?),
            WINDOW_REMOVED => Self::WindowRemoved {
                window: read_window(r)?,
            },
            VIDEO_FRAME => {
                let window = read_window(r)?;
                let seq = r.u64("sequence number")?;
                let codec = CodecId(r.u8("codec")?);
                let keyframe = r.bool("keyframe")?;
                let size = Size::read(r, "frame size")?;
                if !size.is_nonzero() {
                    return Err(r.invalid("frame size", 0));
                }
                Self::VideoFrame(VideoFrame {
                    window,
                    seq,
                    codec,
                    keyframe,
                    size,
                    timestamp_us: r.u64("timestamp")?,
                    payload: r.bytes("payload", MAX_FRAME_PAYLOAD)?,
                })
            }
            CURSOR => Self::Cursor(Cursor {
                window: read_window(r)?,
                shape: CursorShape::read(r)?,
                position: Point::read_opt(r, "cursor position")?,
            }),
            MOVE_RESIZE => Self::MoveResizeStarted(MoveResize {
                window: read_window(r)?,
                edge: Edge::read_opt(r)?,
                grab: Point::read(r, "grab")?,
            }),
            LOCK_CHANGED => Self::LockChanged {
                window: read_window(r)?,
                state: LockState::read(r)?,
            },
            CLIPBOARD => Self::Clipboard {
                text: r.string("clipboard", MAX_CLIPBOARD)?,
            },
            INPUT_HOLDER => Self::InputHolder(match r.bool("holder")? {
                true => Some(Holder {
                    session: SessionId(r.u64("holder session")?),
                    client_id: r.string("holder id", MAX_ID)?,
                    name: r.string("holder name", MAX_NAME)?,
                }),
                false => None,
            }),
            SCREENSHOT => {
                let request = r.u32("request")?;
                let result = match r.u8("screenshot result")? {
                    0 => {
                        let window = read_window(r)?;
                        let size = Size::read(r, "screenshot size")?;
                        if !size.is_nonzero() {
                            return Err(r.invalid("screenshot size", 0));
                        }
                        ScreenshotResult::Image(Screenshot {
                            window,
                            size,
                            codec: CodecId(r.u8("codec")?),
                            payload: r.bytes("payload", MAX_FRAME_PAYLOAD)?,
                        })
                    }
                    1 => ScreenshotResult::NoSuchWindow,
                    2 => ScreenshotResult::NoFrame,
                    3 => ScreenshotResult::Failed(r.string("message", MAX_MESSAGE)?),
                    _ => ScreenshotResult::Failed(String::from("unknown screenshot result")),
                };
                Self::Screenshot { request, result }
            }
            HEARTBEAT => Self::Heartbeat,
            RESIZE_APPLIED => Self::ResizeApplied {
                window: read_window(r)?,
                request: r.u32("request")?,
                from_seq: r.u64("sequence number")?,
            },
            SYNCED => Self::Synced {
                request: r.u32("request")?,
            },
            ACTIVITY => Self::Activity,
            // Only reached if a type gets a limit without a decoder.
            other => return Err(r.invalid("message type", other.into())),
        })
    }
}
