//! Errors of encoding and decoding messages.

/// Why bytes from a peer could not be read as a message.
///
/// Every variant ends the connection: after an error the stream position is
/// unknown, so the reader stops instead of guessing where the next frame starts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The frame header announced a body larger than its type allows.
    #[error("message type {kind:#04x} announces {len} bytes, more than its limit of {max}")]
    TooLarge {
        /// The message type byte.
        kind: u8,
        /// The announced body length.
        len: u32,
        /// The limit for that type.
        max: u32,
    },
    /// A message of the other direction arrived, such as a host message at the host.
    #[error("message type {kind:#04x} travels the other way")]
    WrongDirection {
        /// The message type byte.
        kind: u8,
    },
    /// The body ended before a field it must carry.
    #[error("message type {kind:#04x} ends before its {field} field")]
    Truncated {
        /// The message type byte.
        kind: u8,
        /// The field that was cut.
        field: &'static str,
    },
    /// A string or list is longer than its field allows.
    #[error("{field} in message type {kind:#04x} has {len} entries or bytes, more than {max}")]
    FieldTooLong {
        /// The message type byte.
        kind: u8,
        /// The field.
        field: &'static str,
        /// Its announced length.
        len: usize,
        /// The limit for the field.
        max: usize,
    },
    /// A string field holds bytes that are not UTF-8.
    #[error("{field} in message type {kind:#04x} is not valid UTF-8")]
    InvalidUtf8 {
        /// The message type byte.
        kind: u8,
        /// The field.
        field: &'static str,
    },
    /// A field holds a value outside its range, such as an unknown role.
    #[error("{field} in message type {kind:#04x} has the invalid value {value}")]
    InvalidValue {
        /// The message type byte.
        kind: u8,
        /// The field.
        field: &'static str,
        /// The value read, widened to 64 bits.
        value: u64,
    },
    /// The first message does not start with the protocol's magic bytes, so the
    /// peer does not speak this protocol at all.
    #[error("the peer does not speak the devlanes GUI protocol")]
    BadMagic,
    /// The peer speaks another version of the protocol.
    #[error("the peer speaks GUI protocol version {theirs}, this build speaks {ours}")]
    VersionMismatch {
        /// [`crate::PROTOCOL_VERSION`].
        ours: u16,
        /// The version the peer announced.
        theirs: u16,
    },
}

/// Why a message could not be written. Nothing is written when this is returned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// The encoded body is larger than its type allows.
    #[error("message type {kind:#04x} is {len} bytes, more than its limit of {max}")]
    TooLarge {
        /// The message type byte.
        kind: u8,
        /// The body length.
        len: usize,
        /// The limit for that type.
        max: u32,
    },
    /// A string or list is longer than its field allows.
    #[error("{field} has {len} entries or bytes, more than {max}")]
    FieldTooLong {
        /// The field.
        field: &'static str,
        /// Its length.
        len: usize,
        /// The limit for the field.
        max: usize,
    },
    /// A field holds a value the peer would reject, such as a size above
    /// [`crate::MAX_DIMENSION`] or a wheel delta that is not finite.
    #[error("{field} has a value the protocol does not allow")]
    InvalidValue {
        /// The field.
        field: &'static str,
    },
}
