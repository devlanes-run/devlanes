//! The uncompressed picture codecs take and return.

use std::time::Duration;

use bytes::Bytes;
use devlanes_gui_proto::MAX_DIMENSION;

use crate::CodecError;

/// A BGRA picture: four bytes per pixel, rows `stride` bytes apart, top row
/// first, with the time it was captured. The bytes are shared, so cloning is
/// cheap and a frame can move to an encoder thread without a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    stride: u32,
    data: Bytes,
    timestamp: Duration,
}

impl Frame {
    /// A frame over `data`, checked so every row fits. Sides are 1 to
    /// [`MAX_DIMENSION`], and `stride` is at least `width * 4`. Capture APIs
    /// often pad rows, so `data` may be longer than the last row needs. The
    /// timestamp starts at zero; see [`Frame::with_timestamp`].
    pub fn new(
        width: u32,
        height: u32,
        stride: u32,
        data: impl Into<Bytes>,
    ) -> Result<Self, CodecError> {
        let data = data.into();
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(CodecError::InvalidFrame("a side is zero or too large"));
        }
        let row = width as usize * 4;
        if (stride as usize) < row {
            return Err(CodecError::InvalidFrame("stride is shorter than a row"));
        }
        let needed = (stride as usize)
            .checked_mul(height as usize - 1)
            .and_then(|rows| rows.checked_add(row))
            .ok_or(CodecError::InvalidFrame("the rows do not fit in memory"))?;
        if data.len() < needed {
            return Err(CodecError::InvalidFrame("data is shorter than the rows"));
        }
        Ok(Self {
            width,
            height,
            stride,
            data,
            timestamp: Duration::ZERO,
        })
    }

    /// A frame of tightly packed rows.
    pub fn packed(width: u32, height: u32, data: impl Into<Bytes>) -> Result<Self, CodecError> {
        Self::new(width, height, width.saturating_mul(4), data)
    }

    /// The same frame, captured at `timestamp` on the host's monotonic clock.
    /// Encoders with rate control use it to space frames.
    pub fn with_timestamp(mut self, timestamp: Duration) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// When the frame was captured, on the host's monotonic clock.
    pub fn timestamp(&self) -> Duration {
        self.timestamp
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes from the start of one row to the next.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// The bytes, padding included.
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// The pixels of row `y` without padding, or `None` past the last row.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.stride as usize;
        self.data.get(start..start + self.width as usize * 4)
    }

    /// The rows without padding, shared with this frame when it has none.
    pub fn to_packed(&self) -> Bytes {
        let row = self.width as usize * 4;
        if self.stride as usize == row {
            return self.data.slice(..row * self.height as usize);
        }
        let mut out = Vec::with_capacity(row * self.height as usize);
        for y in 0..self.height {
            out.extend_from_slice(self.row(y).unwrap_or_default());
        }
        Bytes::from(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_past_the_data_are_refused_and_padding_is_dropped() {
        assert!(Frame::new(2, 2, 8, vec![0; 15]).is_err());
        assert!(Frame::new(2, 2, 7, vec![0; 16]).is_err());
        assert!(Frame::new(0, 2, 8, Vec::new()).is_err());
        assert!(Frame::packed(MAX_DIMENSION + 1, 1, Vec::new()).is_err());
        assert!(Frame::new(1, MAX_DIMENSION, u32::MAX, vec![0; 4]).is_err());

        // Two rows of one pixel with four bytes of padding; the last row needs none.
        let frame = Frame::new(1, 2, 8, vec![1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8]).expect("valid");
        assert_eq!(&frame.to_packed()[..], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(frame.row(1), Some(&[5, 6, 7, 8][..]));
        assert_eq!(frame.row(2), None);
    }
}
