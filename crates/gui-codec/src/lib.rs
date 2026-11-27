//! Video codecs of devlanes GUI streaming: the pixel frame, the encoder and
//! decoder traits that each OS backend implements, the `raw` codec every build
//! has, and the selector that picks what both ends share.
//!
//! Encoders and decoders are `Send` and own everything they return, so a host
//! can run its encoder on a thread of its own and a viewer its decoder off the
//! UI thread.

mod frame;
mod raw;
mod select;

pub use devlanes_gui_proto::{CodecId, CodecOffer};
pub use frame::Frame;
pub use raw::{RawBackend, RawDecoder, RawEncoder};
pub use select::{Selection, select};

use bytes::Bytes;

/// Why a codec could not encode, decode or start.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The frame's size, stride or data length do not fit together.
    #[error("invalid frame: {0}")]
    InvalidFrame(&'static str),
    /// The payload does not decode to a picture of the size it claims.
    #[error("the payload is not a {width}x{height} picture")]
    BadPayload {
        /// The width the frame message announced.
        width: u32,
        /// The height the frame message announced.
        height: u32,
    },
    /// The OS library or device behind a codec is missing or failed to start.
    #[error("{codec} is not available: {reason}")]
    Unavailable {
        /// The codec name.
        codec: &'static str,
        /// What is missing, for people.
        reason: String,
    },
    /// The codec library reported an error.
    #[error("{codec} failed")]
    Backend {
        /// The codec name.
        codec: &'static str,
        /// The library's error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// What the encoder should aim for with one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodeParams {
    /// Target bits per second; encoders without rate control ignore it.
    pub bitrate_bps: u32,
    /// Picture quality from 0 to 100, for encoders that have such a knob.
    pub quality: u8,
    /// Whether the output must decode on its own, without earlier frames.
    pub keyframe: bool,
}

/// One encoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// The bytes for [`devlanes_gui_proto::VideoFrame::payload`].
    pub payload: Bytes,
    /// Whether it decodes without earlier frames. A frame at a new size always does.
    pub keyframe: bool,
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
}

/// An encoded frame to decode, as a video frame message carries it.
#[derive(Debug, Clone, Copy)]
pub struct Packet<'a> {
    /// The payload.
    pub payload: &'a [u8],
    /// The picture's width.
    pub width: u32,
    /// The picture's height.
    pub height: u32,
    /// Whether it decodes without earlier frames.
    pub keyframe: bool,
}

/// Turns frames of one window into a stream of one codec.
///
/// One encoder serves one window for as long as it streams. A frame at a new
/// size restarts the stream and comes out as a keyframe.
pub trait VideoEncoder: Send {
    /// The codec and whether this implementation is hardware.
    fn offer(&self) -> CodecOffer;

    /// Encodes `frame`, or returns `None` when it has nothing to send for it,
    /// such as a picture identical to the previous one. Never `None` when
    /// `params.keyframe` is set.
    fn encode(
        &mut self,
        frame: &Frame,
        params: &EncodeParams,
    ) -> Result<Option<Encoded>, CodecError>;
}

/// Turns a stream of one codec back into frames.
pub trait VideoDecoder: Send {
    /// The codec and whether this implementation is hardware.
    fn offer(&self) -> CodecOffer;

    /// Decodes one packet. `None` when the decoder holds the picture back,
    /// which the low-latency decoders devlanes uses do not.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError>;
}

/// A codec implementation a host can create encoders from; the host lists the
/// offers of its backends in its welcome.
pub trait EncoderBackend: Send + Sync {
    /// What encoders of this backend offer.
    fn offer(&self) -> CodecOffer;

    /// A new encoder, for one window.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError>;
}

/// A codec implementation a viewer can create decoders from; the viewer lists
/// the offers of its backends in its hello.
pub trait DecoderBackend: Send + Sync {
    /// What decoders of this backend offer.
    fn offer(&self) -> CodecOffer;

    /// A new decoder, for one window.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError>;
}

// Compiles the README's example with the doc tests, so it keeps matching the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
