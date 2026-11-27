//! The devlanes GUI protocol between a GUI host and its clients, and the pure
//! state machines both ends run on it.
//!
//! # Wire format
//!
//! One TCP connection per client. Each message is a frame: a type byte, a
//! big-endian `u32` body length and the body. Type bytes below `0x80` travel to
//! the host ([`ClientMessage`]) and the others to clients ([`HostMessage`]).
//! Every type has a body limit, checked from the header before the body is
//! buffered. Integers are big-endian, strings are UTF-8 with a `u32` byte
//! length, lists have a `u16` count, and every string and list has a limit.
//!
//! # Compatibility
//!
//! The frame header and the start of the hello, welcome and refusal bodies
//! (magic bytes, then a `u16` [`PROTOCOL_VERSION`]) never change, so a version
//! mismatch is always reported as [`DecodeError::VersionMismatch`]. Within one
//! version, peers stay compatible this way:
//!
//! - A frame of an unknown type is skipped, up to [`MAX_UNKNOWN_BODY`] bytes.
//!   A peer sends a new type only when both hellos list the feature that adds it.
//! - New fields are appended to a body, and readers ignore bytes after the
//!   fields they know. Windows and codec offers carry a length of their own, so
//!   they can grow inside lists.
//! - Codec ids and cursor shapes this build does not know are kept or fall
//!   back to the default arrow. Other enumerations need a feature or a version.
//!
//! The version changes only when an older peer cannot read what a newer one
//! sends.

mod arbiter;
mod error;
mod lock;
mod message;
mod pacer;
mod types;
mod wire;

use std::time::Duration;

pub use arbiter::{HOLD_AFTER_TYPING, InputArbiter, Released, Verdict};
pub use error::{DecodeError, EncodeError};
pub use lock::{RECONNECT_AFTER, RESIZE_TIMEOUT, WAITING_GRACE, WindowLock};
pub use message::{
    ClientMessage, Cursor, Hello, Holder, HostMessage, Key, MAX_CLIPBOARD, MAX_CODECS, MAX_FEATURE,
    MAX_FEATURES, MAX_FRAME_PAYLOAD, MAX_ID, MAX_MESSAGE, MAX_NAME, MAX_TEXT, MAX_WINDOWS,
    MoveResize, Pointer, Refusal, RefusalReason, Screenshot, ScreenshotResult, VideoFrame, Welcome,
    Wheel,
};
pub use pacer::{Encode, MAX_IN_FLIGHT, Pacer, STALL_AFTER, Target};
pub use types::{
    Buttons, CodecId, CodecOffer, CursorImage, CursorShape, Edge, KeyCode, LockState, MAX_APP_ID,
    MAX_CURSOR_SIDE, MAX_DIMENSION, MAX_TITLE, Point, Role, SessionId, Size, WheelUnit, WindowId,
    WindowInfo,
};
pub use wire::{HEADER_LEN, MAX_UNKNOWN_BODY};

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// The protocol version. Both ends must speak the same one.
pub const PROTOCOL_VERSION: u16 = 1;

/// Bytes that start every hello, welcome and refusal body, so a peer that
/// speaks something else on the port fails with [`DecodeError::BadMagic`].
pub const MAGIC: [u8; 4] = *b"DLGU";

/// How often the host sends [`HostMessage::Heartbeat`] to a client it has
/// sent nothing else to, and the shortest gap between two
/// [`HostMessage::Activity`] messages.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
