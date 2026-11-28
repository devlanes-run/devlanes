//! Video codecs of devlanes GUI streaming: the pixel frame, the encoder and
//! decoder traits that each OS backend implements, the `raw` codec every build
//! has, and the selector that picks what both ends share.
//!
//! Encoders and decoders are `Send` and own everything they return, so a host
//! can run its encoder on a thread of its own and a viewer its decoder off the
//! UI thread.
//!
//! The `h264` feature adds the H.264 backends of each OS ([`h264()`]): the
//! system's OpenH264 on Linux, Media Foundation on Windows, the VideoToolbox
//! decoder on macOS. Their FFI lives in `devlanes-sys` and
//! `devlanes-sys-windows`; only `devlanes-gui` enables the feature, so the
//! main `devlanes` binary never carries them.

mod frame;
mod raw;
mod select;
#[cfg(feature = "h264")]
mod yuv;

#[cfg(feature = "h264")]
mod h264;
#[cfg(all(feature = "h264", windows))]
mod media_foundation;
#[cfg(all(feature = "h264", target_os = "linux"))]
mod openh264;
#[cfg(all(feature = "h264", target_os = "macos"))]
mod videotoolbox;

pub use devlanes_gui_proto::{CodecId, CodecOffer};
pub use frame::Frame;
#[cfg(feature = "h264")]
pub use h264::{H264, Unavailable, h264};
#[cfg(all(feature = "h264", windows))]
pub use media_foundation::{MediaFoundationDecoder, MediaFoundationEncoder};
#[cfg(all(feature = "h264", target_os = "linux"))]
pub use openh264::OpenH264;
pub use raw::{RawBackend, RawDecoder, RawEncoder};
pub use select::{Selection, select};
#[cfg(all(feature = "h264", target_os = "macos"))]
pub use videotoolbox::VideoToolbox;

use std::time::Duration;

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

/// What the encoder should aim for with one frame. The values come from the
/// pacer's target and change from frame to frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodeParams {
    /// Target bits per second; encoders without rate control ignore it.
    pub bitrate_bps: u32,
    /// The gap the pacer keeps between frames, which rate control needs to
    /// turn the bitrate into a budget per frame. Frames may still come further
    /// apart; [`Frame::timestamp`] says when each was captured.
    pub frame_interval: Duration,
    /// Picture quality from 0 to 100, for encoders that have such a knob.
    pub quality: u8,
    /// Whether the output must decode on its own, without earlier frames.
    pub keyframe: bool,
    /// A sharp-text refinement: the picture stopped changing and this is
    /// the last one again, to be coded at a high quality so thin text loses
    /// the blur of the lossy frames before it. Lossless codecs see the
    /// previous picture and send nothing.
    pub refine: bool,
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
    /// The capture time from the frame message, copied to the decoded frame.
    pub timestamp: Duration,
}

/// Turns frames of one window into a stream of one codec.
///
/// One encoder serves one window and codec for as long as it streams, shared
/// by the viewers that selected that codec. A frame at a new size restarts the
/// stream and comes out as a keyframe.
pub trait VideoEncoder: Send {
    /// The codec and whether this implementation is hardware.
    fn offer(&self) -> CodecOffer;

    /// Encodes `frame` and returns the output for this frame.
    ///
    /// Synchronous by contract: a backend whose OS API works asynchronously
    /// (a hardware Media Foundation transform, VideoToolbox callbacks) blocks
    /// until the output for this input is ready, so the caller runs encoders
    /// on a thread of its own. Output never belongs to another frame.
    ///
    /// `None` means only that the picture equals the previous one and there
    /// is nothing to send; it is never returned when `params.keyframe` is set.
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

    /// Decodes one packet into its picture, blocking until it is ready, as
    /// [`VideoEncoder::encode`] does. `None` only when the decoder holds the
    /// picture back, which the low-latency decoders devlanes uses do not; the
    /// viewer acknowledges the packet either way.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError>;
}

/// A codec implementation a host can create encoders from; the host lists the
/// offers of its backends in its welcome.
pub trait EncoderBackend: Send + Sync {
    /// What encoders of this backend offer.
    fn offer(&self) -> CodecOffer;

    /// Whether its encoders take frames of `width` by `height`. The host
    /// checks it before encoding, so a window past the limit streams in
    /// another codec without counting against this one. Any size by default.
    fn fits(&self, width: u32, height: u32) -> bool {
        let _ = (width, height);
        true
    }

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
