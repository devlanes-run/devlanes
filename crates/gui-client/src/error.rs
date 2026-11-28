//! Errors of the client.

use devlanes_gui_codec::CodecError;
use devlanes_gui_proto::{CodecId, DecodeError, EncodeError, RefusalReason};

/// Why connecting, sending or a request failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Reading from or writing to the connection failed.
    #[error("GUI connection failed")]
    Io(#[from] std::io::Error),
    /// The host sent bytes that are not a valid message. A host of another
    /// protocol version is [`DecodeError::VersionMismatch`], a peer that does
    /// not speak the protocol [`DecodeError::BadMagic`].
    #[error("the GUI host sent an invalid message")]
    Decode(#[from] DecodeError),
    /// A message could not be encoded, such as text over its limit.
    #[error("the message cannot be sent")]
    Encode(#[from] EncodeError),
    /// The host refused the hello.
    #[error("the GUI host refused the connection: {message}")]
    Refused {
        /// The host's reason.
        reason: RefusalReason,
        /// The host's text for people.
        message: String,
    },
    /// The host did not answer the hello in time.
    #[error("the GUI host did not answer in time")]
    Timeout,
    /// The connection is closed.
    #[error("the GUI connection is closed")]
    Closed,
    /// A screenshot named a window the host does not have, or the host has none.
    #[error("no such window")]
    NoSuchWindow,
    /// The window has not drawn a frame yet.
    #[error("the window has not drawn yet")]
    NoFrame,
    /// The host could not take the screenshot.
    #[error("the screenshot failed: {0}")]
    ScreenshotFailed(String),
    /// A payload is in a codec this client cannot decode.
    #[error("the payload uses codec {0:?}, which this client cannot decode")]
    UnsupportedCodec(CodecId),
    /// Decoding a payload failed.
    #[error("decoding failed")]
    Codec(#[from] CodecError),
    /// The host does not implement a feature the call needs.
    #[error("the GUI host does not support {0}")]
    Unsupported(&'static str),
}
