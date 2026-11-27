//! Values that messages carry: ids, geometry, codecs, windows, cursors.

use bytes::Bytes;

use crate::error::{DecodeError, EncodeError};
use crate::wire::{Reader, Writer};

/// Largest window or frame width and height. It bounds the memory a decoded
/// frame takes (8192 x 8192 BGRA is 256 MiB) and is above 8K displays.
pub const MAX_DIMENSION: u32 = 8192;

/// A window of the service, unique among the host's windows for the life of
/// the host process. Ids are not reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

/// One connection to a host, assigned by the host and sent in the welcome.
/// Unlike a client id, which a client picks and two clients may share, it is
/// unique per host process. The pacer and the input arbiter key their state by
/// it, the input holder notice names it, and a later input connection will
/// name the session it belongs to with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// The secret the daemon gives a host for its control connection.
///
/// `Debug` prints no secret and comparison takes the same time wherever the
/// bytes differ. The daemon passes it in the host's environment; the host
/// removes that variable from its own environment before it starts a child
/// of its own, such as Xwayland, so no other process can read it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// A token holding `secret`.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The secret, for writing it to the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    /// Prints a placeholder instead of the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

impl PartialEq for Token {
    /// Compares in constant time for tokens of equal length; the length is not secret.
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq;
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

impl Eq for Token {}

/// A position in window pixels, or a movement in screen pixels. Negative
/// values are allowed, since a held pointer can leave the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    /// Pixels to the right of the origin.
    pub x: i32,
    /// Pixels below the origin.
    pub y: i32,
}

impl Point {
    /// A point at `x`, `y`.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Reads two `i32`.
    pub(crate) fn read(r: &mut Reader, field: &'static str) -> Result<Self, DecodeError> {
        Ok(Self {
            x: r.i32(field)?,
            y: r.i32(field)?,
        })
    }

    /// Writes two `i32`.
    pub(crate) fn write(self, w: &mut Writer<'_>) {
        w.i32(self.x);
        w.i32(self.y);
    }

    /// Reads a presence byte and, when present, a point.
    pub(crate) fn read_opt(
        r: &mut Reader,
        field: &'static str,
    ) -> Result<Option<Self>, DecodeError> {
        Ok(match r.bool(field)? {
            true => Some(Self::read(r, field)?),
            false => None,
        })
    }

    /// Writes a presence byte and, when present, a point.
    pub(crate) fn write_opt(value: Option<Self>, w: &mut Writer<'_>) {
        w.bool(value.is_some());
        if let Some(point) = value {
            point.write(w);
        }
    }
}

/// A size in pixels, each side at most [`MAX_DIMENSION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Size {
    /// A size of `width` by `height`.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Whether both sides are within [`MAX_DIMENSION`].
    pub const fn is_valid(self) -> bool {
        self.width <= MAX_DIMENSION && self.height <= MAX_DIMENSION
    }

    /// Whether neither side is zero, as a video frame needs.
    pub const fn is_nonzero(self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Reads two `u32` and checks them against [`MAX_DIMENSION`].
    pub(crate) fn read(r: &mut Reader, field: &'static str) -> Result<Self, DecodeError> {
        let size = Self {
            width: r.u32(field)?,
            height: r.u32(field)?,
        };
        if !size.is_valid() {
            return Err(r.invalid(field, size.width.max(size.height).into()));
        }
        Ok(size)
    }

    /// Writes two `u32`, refusing a size above [`MAX_DIMENSION`].
    pub(crate) fn write(self, w: &mut Writer<'_>, field: &'static str) -> Result<(), EncodeError> {
        if !self.is_valid() {
            return Err(EncodeError::InvalidValue { field });
        }
        w.u32(self.width);
        w.u32(self.height);
        Ok(())
    }

    /// Reads a presence byte and, when present, a size.
    pub(crate) fn read_opt(
        r: &mut Reader,
        field: &'static str,
    ) -> Result<Option<Self>, DecodeError> {
        Ok(match r.bool(field)? {
            true => Some(Self::read(r, field)?),
            false => None,
        })
    }

    /// Writes a presence byte and, when present, a size.
    pub(crate) fn write_opt(
        value: Option<Self>,
        w: &mut Writer<'_>,
        field: &'static str,
    ) -> Result<(), EncodeError> {
        w.bool(value.is_some());
        match value {
            Some(size) => size.write(w, field),
            None => Ok(()),
        }
    }
}

/// A video codec, as one byte on the wire.
///
/// Unknown values are kept, so a hello listing a codec a newer peer added still
/// decodes; the selector skips codecs it does not rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodecId(pub u8);

impl CodecId {
    /// Full BGRA frames compressed with zstd. Every build has it.
    pub const RAW: Self = Self(1);
    /// H.264 byte stream (Annex B).
    pub const H264: Self = Self(2);

    /// The codec's name in notes and logs, or `None` for one this build does not know.
    pub const fn name(self) -> Option<&'static str> {
        match self {
            Self::RAW => Some("raw"),
            Self::H264 => Some("h264"),
            _ => None,
        }
    }
}

/// A codec one end can encode or decode, and whether its implementation is
/// hardware. A host lists its encoders and a viewer its decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodecOffer {
    /// The codec.
    pub codec: CodecId,
    /// Whether a hardware encoder or decoder implements it.
    pub hardware: bool,
}

impl CodecOffer {
    /// Reads an offer as a nested struct, so later versions can add fields.
    pub(crate) fn read(r: &mut Reader) -> Result<Self, DecodeError> {
        let mut r = r.nested("codec offer")?;
        let codec = CodecId(r.u8("codec")?);
        let flags = r.u8("codec flags")?;
        Ok(Self {
            codec,
            hardware: flags & 1 != 0,
        })
    }

    /// Writes an offer as a nested struct.
    pub(crate) fn write(self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        w.nested("codec offer", |w| {
            w.u8(self.codec.0);
            w.u8(self.hardware.into());
            Ok(())
        })
    }
}

/// What a connection is for, sent in the hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// A viewer: receives windows and video, sends input.
    Viewer,
    /// A command-line or MCP tool: screenshots and input, no video stream.
    Tool,
    /// The daemon, with the token it gave the host: unit identity, exit,
    /// freezing; receives viewer activity.
    Control,
    /// Reserved for a later second connection that carries input,
    /// acknowledgements and small messages for an existing viewer session, so
    /// they stay responsive while video is behind. Hosts of this version refuse it.
    Input,
    /// A role this build does not know, kept so the host can answer
    /// [`crate::RefusalReason::RoleNotSupported`].
    Unknown(u8),
}

impl Role {
    /// The wire value.
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            Self::Viewer => 1,
            Self::Tool => 2,
            Self::Control => 3,
            Self::Input => 4,
            Self::Unknown(value) => value,
        }
    }

    /// The role of a wire value.
    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Viewer,
            2 => Self::Tool,
            3 => Self::Control,
            4 => Self::Input,
            other => Self::Unknown(other),
        }
    }
}

/// Pointer buttons held, as a bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Buttons(pub u8);

impl Buttons {
    /// No button held.
    pub const NONE: Self = Self(0);
    /// The primary button.
    pub const LEFT: Self = Self(1);
    /// The secondary button.
    pub const RIGHT: Self = Self(1 << 1);
    /// The wheel button.
    pub const MIDDLE: Self = Self(1 << 2);
    /// The back side button.
    pub const BACK: Self = Self(1 << 3);
    /// The forward side button.
    pub const FORWARD: Self = Self(1 << 4);

    /// Whether no button is held.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every button of `other` is held.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The buttons held in `self` but not in `before`.
    pub const fn pressed_since(self, before: Self) -> Self {
        Self(self.0 & !before.0)
    }
}

impl std::ops::BitOr for Buttons {
    type Output = Self;

    /// Both sets of buttons.
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A physical key as a USB HID usage: the usage page in the high 16 bits and
/// the usage ID in the low 16, such as page 0x07 usage 0x04 for the key
/// labelled A on a US layout, or page 0x0C usage 0xE9 for volume up. Every
/// viewer OS has a table from its scan codes to these, and the host maps them
/// back, so the key means the same position on every keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCode(pub u32);

impl KeyCode {
    /// The keyboard usage page: letters, digits, modifiers, function keys.
    pub const KEYBOARD_PAGE: u16 = 0x07;
    /// The consumer usage page: media, volume, browser keys.
    pub const CONSUMER_PAGE: u16 = 0x0c;

    /// The key with `usage` on `page`.
    pub const fn new(page: u16, usage: u16) -> Self {
        Self(((page as u32) << 16) | usage as u32)
    }

    /// A key on the keyboard page.
    pub const fn keyboard(usage: u16) -> Self {
        Self::new(Self::KEYBOARD_PAGE, usage)
    }

    /// The usage page.
    pub const fn page(self) -> u16 {
        (self.0 >> 16) as u16
    }

    /// The usage ID within the page.
    pub const fn usage(self) -> u16 {
        self.0 as u16
    }
}

/// The unit of a wheel delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelUnit {
    /// Pixels, from touchpads and precise wheels.
    Pixels,
    /// Lines, from notched wheels.
    Lines,
}

/// Why a window is locked. Input is dropped while a window is locked, and the
/// viewer dims its last frame with one line of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockState {
    /// Not locked.
    Unlocked,
    /// A size change is under way; ends with the first frame after it.
    Resizing,
    /// The window closed while the service runs, such as an app restarting.
    Waiting,
    /// The connection to the host dropped. Only viewers set it; hosts never send it.
    Reconnecting,
    /// The lane is frozen for inactivity. A click from a viewer still reaches
    /// the host, which reports it as activity so the lane thaws; the app does
    /// not get it.
    Frozen,
    /// The window is gone for good: the service stopped or failed, which is
    /// final for this host process (a unit restart starts a new host and the
    /// viewer connects again), or it stayed closed past
    /// [`crate::WAITING_GRACE`], after which a new window of the service can
    /// still appear as a window of its own.
    Closed,
    /// Locked for a reason this build does not know, from a newer host. Shown
    /// with generic text.
    Locked,
}

impl LockState {
    /// Wire value of [`LockState::Locked`]; decoders map every unknown value to it.
    const LOCKED: u8 = 0xff;

    /// The wire value.
    pub(crate) const fn to_u8(self) -> u8 {
        match self {
            Self::Unlocked => 0,
            Self::Resizing => 1,
            Self::Waiting => 2,
            Self::Reconnecting => 3,
            Self::Frozen => 4,
            Self::Closed => 5,
            Self::Locked => Self::LOCKED,
        }
    }

    /// Reads one byte as a lock state. Unknown values are [`LockState::Locked`],
    /// so a host can add states without breaking older viewers.
    pub(crate) fn read(r: &mut Reader) -> Result<Self, DecodeError> {
        Ok(match r.u8("lock state")? {
            0 => Self::Unlocked,
            1 => Self::Resizing,
            2 => Self::Waiting,
            3 => Self::Reconnecting,
            4 => Self::Frozen,
            5 => Self::Closed,
            _ => Self::Locked,
        })
    }

    /// Whether input is dropped in this state.
    pub const fn is_locked(self) -> bool {
        !matches!(self, Self::Unlocked)
    }
}

/// Window title length limit, in bytes.
pub const MAX_TITLE: usize = 1024;
/// App id length limit, in bytes.
pub const MAX_APP_ID: usize = 256;

/// What sort of window it is, which decides how a viewer shows it. Unknown
/// values decode as [`WindowKind::Toplevel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WindowKind {
    /// A main window: framed as the frame rules say, movable, focusable.
    #[default]
    Toplevel,
    /// A dialog of its parent: also framed and movable, kept above the parent.
    Dialog,
    /// A menu or dropdown: placed at its offset from the parent, no frame, no focus.
    Popup,
    /// A tooltip: like a popup, and it takes no input.
    Tooltip,
}

impl WindowKind {
    /// The wire value.
    const fn to_u8(self) -> u8 {
        match self {
            Self::Toplevel => 0,
            Self::Dialog => 1,
            Self::Popup => 2,
            Self::Tooltip => 3,
        }
    }

    /// The kind of a wire value.
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Dialog,
            2 => Self::Popup,
            3 => Self::Tooltip,
            _ => Self::Toplevel,
        }
    }
}

/// One window of the service, as the host describes it.
///
/// Its size and every coordinate about it use the window's geometry: the
/// rectangle the app calls its window, without the shadow toolkits draw
/// around it and including a title bar the app draws above its main surface.
/// The host crops frames to that rectangle, and pointer positions, popup
/// offsets and grab points have their origin at its top-left corner.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowInfo {
    /// The window's id.
    pub id: WindowId,
    /// Its title, at most [`MAX_TITLE`] bytes.
    pub title: String,
    /// The app id (Wayland app id, X11 `WM_CLASS`, Windows AppUserModelID) or
    /// empty, at most [`MAX_APP_ID`] bytes.
    pub app_id: String,
    /// Its current size in pixels; zero while the app has not drawn yet.
    pub size: Size,
    /// The smallest size the app accepts, when it says.
    pub min_size: Option<Size>,
    /// The largest size the app accepts, when it says.
    pub max_size: Option<Size>,
    /// Whether the app draws its own title bar and border, so the viewer shows
    /// no frame of its own.
    pub draws_own_frame: bool,
    /// Whether the host reports when the app starts a move or resize, so a
    /// borderless viewer window can start a local drag.
    pub reports_moves: bool,
    /// For a popup or child, the window it belongs to.
    pub parent: Option<WindowId>,
    /// For a popup, its top-left corner relative to the parent's.
    pub offset: Point,
    /// Why the window is locked, if it is.
    pub lock: LockState,
    /// What sort of window it is.
    pub kind: WindowKind,
}

impl WindowInfo {
    /// Reads a window as a nested struct, so later versions can add fields.
    pub(crate) fn read(r: &mut Reader) -> Result<Self, DecodeError> {
        let mut r = r.nested("window")?;
        let id = WindowId(r.u32("window id")?);
        let title = r.string("title", MAX_TITLE)?;
        let app_id = r.string("app id", MAX_APP_ID)?;
        let size = Size::read(&mut r, "size")?;
        let min_size = Size::read_opt(&mut r, "minimum size")?;
        let max_size = Size::read_opt(&mut r, "maximum size")?;
        let flags = r.u8("window flags")?;
        let parent = match r.bool("parent")? {
            true => Some(WindowId(r.u32("parent")?)),
            false => None,
        };
        let offset = Point::read(&mut r, "offset")?;
        let lock = LockState::read(&mut r)?;
        let kind = WindowKind::from_u8(r.u8("window kind")?);
        Ok(Self {
            id,
            title,
            app_id,
            size,
            min_size,
            max_size,
            draws_own_frame: flags & 1 != 0,
            reports_moves: flags & 2 != 0,
            parent,
            offset,
            lock,
            kind,
        })
    }

    /// Writes a window as a nested struct.
    pub(crate) fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        w.nested("window", |w| {
            w.u32(self.id.0);
            w.string("title", &self.title, MAX_TITLE)?;
            w.string("app id", &self.app_id, MAX_APP_ID)?;
            self.size.write(w, "size")?;
            Size::write_opt(self.min_size, w, "minimum size")?;
            Size::write_opt(self.max_size, w, "maximum size")?;
            w.u8(u8::from(self.draws_own_frame) | (u8::from(self.reports_moves) << 1));
            w.bool(self.parent.is_some());
            if let Some(parent) = self.parent {
                w.u32(parent.0);
            }
            self.offset.write(w);
            w.u8(self.lock.to_u8());
            w.u8(self.kind.to_u8());
            Ok(())
        })
    }
}

/// Largest custom cursor image side.
pub const MAX_CURSOR_SIDE: u32 = 256;

/// A cursor the app set, by name where the viewer OS has a matching cursor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// The arrow. Shapes this build does not know decode as this one.
    Default,
    /// A hand over a link.
    Pointer,
    /// The text I-beam.
    Text,
    /// A crosshair.
    Crosshair,
    /// Four arrows, for moving.
    Move,
    /// An open hand.
    Grab,
    /// A closed hand.
    Grabbing,
    /// A forbidden sign.
    NotAllowed,
    /// Busy, input blocked.
    Wait,
    /// Busy, input still accepted.
    Progress,
    /// A question mark.
    Help,
    /// Resize left and right.
    EwResize,
    /// Resize up and down.
    NsResize,
    /// Resize along the top-right to bottom-left diagonal.
    NeswResize,
    /// Resize along the top-left to bottom-right diagonal.
    NwseResize,
    /// Resize a column.
    ColResize,
    /// Resize a row.
    RowResize,
    /// No cursor.
    Hidden,
    /// A picture the app drew.
    Image(CursorImage),
}

/// A custom cursor picture.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorImage {
    /// Size, each side 1 to [`MAX_CURSOR_SIDE`].
    pub size: Size,
    /// The point of the picture that is the pointer's position.
    pub hotspot: Point,
    /// Tightly packed BGRA rows with premultiplied alpha, `width * height * 4` bytes.
    pub pixels: Bytes,
}

impl CursorImage {
    /// The byte length of the pixels, or `None` when the size is out of range.
    fn pixel_len(size: Size) -> Option<usize> {
        let fits =
            size.is_nonzero() && size.width <= MAX_CURSOR_SIDE && size.height <= MAX_CURSOR_SIDE;
        fits.then(|| (size.width * size.height * 4) as usize)
    }
}

impl CursorShape {
    /// Shapes in wire order; a shape's wire value is its index plus one.
    const NAMED: [CursorShape; 18] = [
        Self::Default,
        Self::Pointer,
        Self::Text,
        Self::Crosshair,
        Self::Move,
        Self::Grab,
        Self::Grabbing,
        Self::NotAllowed,
        Self::Wait,
        Self::Progress,
        Self::Help,
        Self::EwResize,
        Self::NsResize,
        Self::NeswResize,
        Self::NwseResize,
        Self::ColResize,
        Self::RowResize,
        Self::Hidden,
    ];

    /// Wire value of a custom image.
    const IMAGE: u8 = 0xff;

    /// Reads a shape. Unknown names fall back to [`CursorShape::Default`], so a
    /// host can add names without a version change.
    pub(crate) fn read(r: &mut Reader) -> Result<Self, DecodeError> {
        let value = r.u8("cursor shape")?;
        if value == Self::IMAGE {
            let size = Size::read(r, "cursor size")?;
            let Some(expected) = CursorImage::pixel_len(size) else {
                return Err(r.invalid("cursor size", size.width.max(size.height).into()));
            };
            let hotspot = Point::read(r, "cursor hotspot")?;
            let pixels = r.bytes("cursor pixels", expected)?;
            if pixels.len() != expected {
                return Err(r.invalid("cursor pixels", pixels.len() as u64));
            }
            return Ok(Self::Image(CursorImage {
                size,
                hotspot,
                pixels,
            }));
        }
        let index = usize::from(value).wrapping_sub(1);
        Ok(Self::NAMED.get(index).cloned().unwrap_or(Self::Default))
    }

    /// Writes a shape.
    pub(crate) fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        if let Self::Image(image) = self {
            let Some(expected) = CursorImage::pixel_len(image.size) else {
                return Err(EncodeError::InvalidValue {
                    field: "cursor size",
                });
            };
            if image.pixels.len() != expected {
                return Err(EncodeError::InvalidValue {
                    field: "cursor pixels",
                });
            }
            w.u8(Self::IMAGE);
            image.size.write(w, "cursor size")?;
            image.hotspot.write(w);
            return w.bytes("cursor pixels", &image.pixels, expected);
        }
        let index = Self::NAMED
            .iter()
            .position(|named| named == self)
            .unwrap_or(0);
        w.u8(index as u8 + 1);
        Ok(())
    }
}

/// The edge or corner an interactive resize started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    /// Top edge.
    Top,
    /// Bottom edge.
    Bottom,
    /// Left edge.
    Left,
    /// Right edge.
    Right,
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
}

impl Edge {
    /// Edges in wire order; an edge's wire value is its index plus one, and 0
    /// means a move.
    const ALL: [Edge; 8] = [
        Self::Top,
        Self::Bottom,
        Self::Left,
        Self::Right,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// Reads `None` for a move or the edge of a resize.
    pub(crate) fn read_opt(r: &mut Reader) -> Result<Option<Self>, DecodeError> {
        match r.u8("edge")? {
            0 => Ok(None),
            value => match Self::ALL.get(usize::from(value) - 1) {
                Some(edge) => Ok(Some(*edge)),
                None => Err(r.invalid("edge", value.into())),
            },
        }
    }

    /// Writes `None` as a move.
    pub(crate) fn write_opt(value: Option<Self>, w: &mut Writer<'_>) {
        let byte = match value {
            None => 0,
            Some(edge) => Self::ALL.iter().position(|e| *e == edge).unwrap_or(0) as u8 + 1,
        };
        w.u8(byte);
    }
}
