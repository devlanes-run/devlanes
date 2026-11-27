//! Frames and the byte-level reading and writing of message bodies.
//!
//! A frame is a type byte, a big-endian `u32` body length and the body. Integers
//! are big-endian, strings and byte strings carry a `u32` length, lists a `u16`
//! count, and structs that may grow (windows, codec offers) a `u16` length of
//! their own so a reader can skip fields it does not know.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::error::{DecodeError, EncodeError};

/// Bytes before every body: the type byte and the body length.
pub const HEADER_LEN: usize = 5;

/// Largest body of a message type this build does not know. Such messages are
/// skipped, and their body is held until it is complete, so the limit bounds
/// that memory.
pub const MAX_UNKNOWN_BODY: u32 = 1024 * 1024;

/// Which end reads a stream of frames. Type bytes below `0x80` travel to the
/// host and the others to its clients, so a frame sent the wrong way is caught
/// before its body is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    ToHost,
    ToClient,
}

impl Direction {
    /// Whether frames of this type byte travel in this direction.
    fn carries(self, kind: u8) -> bool {
        (kind < 0x80) == (self == Direction::ToHost)
    }
}

/// Takes the next complete frame of a known type out of `buf`.
///
/// Returns `None` while the frame is incomplete. Each call looks only at the
/// header and consumes whole frames, so feeding a stream in small chunks costs
/// time linear in its size. A body is never buffered beyond the limit of its
/// type, and frames of unknown types are dropped once complete.
pub(crate) fn next_frame(
    buf: &mut BytesMut,
    direction: Direction,
    limit: fn(u8) -> Option<u32>,
) -> Result<Option<(u8, Bytes)>, DecodeError> {
    loop {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let kind = buf[0];
        let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        if !direction.carries(kind) {
            return Err(DecodeError::WrongDirection { kind });
        }
        let known = limit(kind);
        let max = known.unwrap_or(MAX_UNKNOWN_BODY);
        if len > max {
            return Err(DecodeError::TooLarge { kind, len, max });
        }
        let total = HEADER_LEN + len as usize;
        if buf.len() < total {
            return Ok(None);
        }
        let mut frame = buf.split_to(total);
        if known.is_none() {
            continue;
        }
        frame.advance(HEADER_LEN);
        return Ok(Some((kind, frame.freeze())));
    }
}

/// Appends one frame whose body `body` writes, or nothing when it fails or
/// exceeds `max`.
pub(crate) fn write_frame(
    out: &mut BytesMut,
    kind: u8,
    max: u32,
    body: impl FnOnce(&mut Writer<'_>) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    let start = out.len();
    out.put_u8(kind);
    out.put_u32(0);
    let result = body(&mut Writer { out });
    let len = out.len() - start - HEADER_LEN;
    let result = match result {
        Ok(()) if len > max as usize => Err(EncodeError::TooLarge { kind, len, max }),
        other => other,
    };
    match result {
        Ok(()) => {
            out[start + 1..start + HEADER_LEN].copy_from_slice(&(len as u32).to_be_bytes());
            Ok(())
        }
        Err(error) => {
            out.truncate(start);
            Err(error)
        }
    }
}

/// Reads the fields of one body in order. Bytes left after the last field this
/// build knows are ignored: that is where later versions append fields.
pub(crate) struct Reader {
    kind: u8,
    body: Bytes,
}

impl Reader {
    /// A reader over the body of a message of type `kind`.
    pub(crate) fn new(kind: u8, body: Bytes) -> Self {
        Self { kind, body }
    }

    /// The type byte, for errors raised by the caller.
    pub(crate) fn kind(&self) -> u8 {
        self.kind
    }

    /// Splits off the next `n` bytes, or fails naming `field`.
    fn take(&mut self, n: usize, field: &'static str) -> Result<Bytes, DecodeError> {
        if self.body.len() < n {
            return Err(DecodeError::Truncated {
                kind: self.kind,
                field,
            });
        }
        Ok(self.body.split_to(n))
    }

    /// A fixed-size big-endian field.
    fn array<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], DecodeError> {
        let bytes = self.take(N, field)?;
        let mut out = [0; N];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// An error for a value out of range.
    pub(crate) fn invalid(&self, field: &'static str, value: u64) -> DecodeError {
        DecodeError::InvalidValue {
            kind: self.kind,
            field,
            value,
        }
    }

    /// One byte.
    pub(crate) fn u8(&mut self, field: &'static str) -> Result<u8, DecodeError> {
        Ok(self.array::<1>(field)?[0])
    }

    /// A big-endian `u16`.
    pub(crate) fn u16(&mut self, field: &'static str) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array(field)?))
    }

    /// A big-endian `u32`.
    pub(crate) fn u32(&mut self, field: &'static str) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array(field)?))
    }

    /// A big-endian `u64`.
    pub(crate) fn u64(&mut self, field: &'static str) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array(field)?))
    }

    /// A big-endian two's complement `i32`.
    pub(crate) fn i32(&mut self, field: &'static str) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.array(field)?))
    }

    /// A finite IEEE 754 `f32`, sent as its big-endian bits.
    pub(crate) fn f32(&mut self, field: &'static str) -> Result<f32, DecodeError> {
        let bits = self.u32(field)?;
        let value = f32::from_bits(bits);
        if !value.is_finite() {
            return Err(self.invalid(field, bits.into()));
        }
        Ok(value)
    }

    /// A byte that must be 0 or 1.
    pub(crate) fn bool(&mut self, field: &'static str) -> Result<bool, DecodeError> {
        match self.u8(field)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.invalid(field, other.into())),
        }
    }

    /// A length checked against `max` before anything is taken.
    fn len(&mut self, len: usize, field: &'static str, max: usize) -> Result<usize, DecodeError> {
        if len > max {
            return Err(DecodeError::FieldTooLong {
                kind: self.kind,
                field,
                len,
                max,
            });
        }
        Ok(len)
    }

    /// A byte string with a `u32` length of at most `max`, shared with the body
    /// instead of copied.
    pub(crate) fn bytes(&mut self, field: &'static str, max: usize) -> Result<Bytes, DecodeError> {
        let len = self.u32(field)? as usize;
        let len = self.len(len, field, max)?;
        self.take(len, field)
    }

    /// A UTF-8 string with a `u32` byte length of at most `max`.
    pub(crate) fn string(
        &mut self,
        field: &'static str,
        max: usize,
    ) -> Result<String, DecodeError> {
        let bytes = self.bytes(field, max)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::InvalidUtf8 {
            kind: self.kind,
            field,
        })
    }

    /// A `u16` list count of at most `max`.
    pub(crate) fn count(&mut self, field: &'static str, max: usize) -> Result<usize, DecodeError> {
        let count = self.u16(field)? as usize;
        self.len(count, field, max)
    }

    /// A struct with a `u16` length of its own, read through its own reader so
    /// fields appended to it later are skipped.
    pub(crate) fn nested(&mut self, field: &'static str) -> Result<Reader, DecodeError> {
        let len = self.u16(field)? as usize;
        let body = self.take(len, field)?;
        Ok(Reader::new(self.kind, body))
    }
}

/// Writes the fields of one body in order.
pub(crate) struct Writer<'a> {
    out: &'a mut BytesMut,
}

impl Writer<'_> {
    /// One byte.
    pub(crate) fn u8(&mut self, value: u8) {
        self.out.put_u8(value);
    }

    /// A big-endian `u16`.
    pub(crate) fn u16(&mut self, value: u16) {
        self.out.put_u16(value);
    }

    /// A big-endian `u32`.
    pub(crate) fn u32(&mut self, value: u32) {
        self.out.put_u32(value);
    }

    /// A big-endian `u64`.
    pub(crate) fn u64(&mut self, value: u64) {
        self.out.put_u64(value);
    }

    /// A big-endian `i32`.
    pub(crate) fn i32(&mut self, value: i32) {
        self.out.put_i32(value);
    }

    /// A finite `f32` as its bits; NaN and infinities are refused.
    pub(crate) fn f32(&mut self, field: &'static str, value: f32) -> Result<(), EncodeError> {
        if !value.is_finite() {
            return Err(EncodeError::InvalidValue { field });
        }
        self.out.put_u32(value.to_bits());
        Ok(())
    }

    /// A byte of 0 or 1.
    pub(crate) fn bool(&mut self, value: bool) {
        self.out.put_u8(value.into());
    }

    /// Fails when `len` is above `max`.
    fn check(field: &'static str, len: usize, max: usize) -> Result<(), EncodeError> {
        if len > max {
            return Err(EncodeError::FieldTooLong { field, len, max });
        }
        Ok(())
    }

    /// A byte string with a `u32` length of at most `max`.
    pub(crate) fn bytes(
        &mut self,
        field: &'static str,
        value: &[u8],
        max: usize,
    ) -> Result<(), EncodeError> {
        Self::check(field, value.len(), max)?;
        let len = u32::try_from(value.len()).map_err(|_| EncodeError::FieldTooLong {
            field,
            len: value.len(),
            max,
        })?;
        self.out.put_u32(len);
        self.out.put_slice(value);
        Ok(())
    }

    /// A UTF-8 string with a `u32` byte length of at most `max`.
    pub(crate) fn string(
        &mut self,
        field: &'static str,
        value: &str,
        max: usize,
    ) -> Result<(), EncodeError> {
        self.bytes(field, value.as_bytes(), max)
    }

    /// A `u16` list count of at most `max`.
    pub(crate) fn count(
        &mut self,
        field: &'static str,
        count: usize,
        max: usize,
    ) -> Result<(), EncodeError> {
        Self::check(field, count, max.min(u16::MAX.into()))?;
        self.out.put_u16(count as u16);
        Ok(())
    }

    /// A struct with a `u16` length of its own, which `body` writes.
    pub(crate) fn nested(
        &mut self,
        field: &'static str,
        body: impl FnOnce(&mut Writer<'_>) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        let start = self.out.len();
        self.out.put_u16(0);
        body(&mut Writer { out: self.out })?;
        let len = self.out.len() - start - 2;
        let len = u16::try_from(len).map_err(|_| EncodeError::FieldTooLong {
            field,
            len,
            max: u16::MAX.into(),
        })?;
        self.out[start..start + 2].copy_from_slice(&len.to_be_bytes());
        Ok(())
    }
}
