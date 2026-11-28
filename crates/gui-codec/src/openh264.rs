//! H.264 on Linux through the system's OpenH264, loaded at run time by
//! `devlanes_sys::openh264`. Software only, for the host and the viewer.

use std::sync::Arc;

use bytes::Bytes;
use devlanes_gui_proto::{CodecId, CodecOffer};
use devlanes_sys::openh264::{
    Decoder, Encoder, EncoderConfig, FrameKind, I420, Library, LoadError,
};

use crate::h264::frame_rate;
use crate::yuv::{self, I420Buffer, Planes};
use crate::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, VideoDecoder,
    VideoEncoder,
};

/// What OpenH264 encoders and decoders offer.
const OFFER: CodecOffer = CodecOffer {
    codec: CodecId::H264,
    hardware: false,
};

/// The quantizer of a sharp-text refinement: fine enough that thin text keeps
/// its edges, coarser than lossless so one frame stays a modest size.
const REFINE_QP: u8 = 16;

/// Wraps an OpenH264 error.
fn backend(source: devlanes_sys::openh264::Error) -> CodecError {
    CodecError::Backend {
        codec: "h264",
        source: Box::new(source),
    }
}

/// The system's OpenH264, as an encoder and a decoder backend.
#[derive(Debug, Clone)]
pub struct OpenH264 {
    library: Arc<Library>,
}

impl OpenH264 {
    /// Loads the system's OpenH264.
    pub fn load() -> Result<Self, LoadError> {
        Library::load().map(|library| Self { library })
    }

    /// Loads OpenH264 from the first of `candidates` that loads, names the
    /// loader searches or paths.
    pub fn load_from<S: AsRef<std::ffi::OsStr>>(candidates: &[S]) -> Result<Self, LoadError> {
        Library::load_from(candidates).map(|library| Self { library })
    }

    /// The loaded version, such as `2.6.0`.
    pub fn version(&self) -> String {
        self.library.version().to_string()
    }
}

impl EncoderBackend for OpenH264 {
    /// H.264 in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// An encoder that starts its OpenH264 encoder on the first frame, at
    /// that frame's size.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Ok(Box::new(OpenH264Encoder {
            library: self.library.clone(),
            encoder: None,
            picture: I420Buffer::default(),
            output: Vec::new(),
        }))
    }
}

impl DecoderBackend for OpenH264 {
    /// H.264 in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// A decoder.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(OpenH264Decoder {
            decoder: Decoder::new(&self.library).map_err(backend)?,
        }))
    }
}

/// One window's OpenH264 stream.
struct OpenH264Encoder {
    library: Arc<Library>,
    /// Started on the first frame and again when the size changes.
    encoder: Option<Encoder>,
    picture: I420Buffer,
    output: Vec<u8>,
}

impl VideoEncoder for OpenH264Encoder {
    /// H.264 in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// Encodes `frame` padded to even sides; a new size starts a new encoder,
    /// whose first frame is an IDR frame. A refinement is coded at a fixed
    /// fine quantizer, then bitrate control resumes.
    fn encode(
        &mut self,
        frame: &Frame,
        params: &EncodeParams,
    ) -> Result<Option<Encoded>, CodecError> {
        self.picture.fill(frame);
        let (width, height) = (self.picture.width, self.picture.height);
        let rate = frame_rate(params);
        let bitrate = params.bitrate_bps.max(1);
        let fresh = !self
            .encoder
            .as_ref()
            .is_some_and(|e| (e.config().width, e.config().height) == (width, height));
        if fresh {
            self.encoder = None;
            let config = EncoderConfig {
                width,
                height,
                bitrate_bps: bitrate,
                frame_rate: rate,
            };
            self.encoder = Some(Encoder::new(&self.library, config).map_err(backend)?);
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Err(CodecError::InvalidFrame("no encoder"));
        };
        if !fresh {
            let config = encoder.config();
            if config.bitrate_bps != bitrate {
                encoder.set_bitrate(bitrate).map_err(backend)?;
            }
            if (config.frame_rate - rate).abs() > 0.5 {
                encoder.set_frame_rate(rate).map_err(backend)?;
            }
            if params.keyframe {
                encoder.force_idr().map_err(backend)?;
            }
        }
        if params.refine {
            encoder.set_fixed_qp(Some(REFINE_QP)).map_err(backend)?;
        }
        let picture = I420 {
            width,
            height,
            y: &self.picture.y,
            u: &self.picture.u,
            v: &self.picture.v,
            y_stride: width,
            uv_stride: width / 2,
        };
        self.output.clear();
        let timestamp = i64::try_from(frame.timestamp().as_millis()).unwrap_or(i64::MAX);
        let encoded = encoder.encode(&picture, timestamp, &mut self.output);
        if params.refine {
            encoder.set_fixed_qp(None).map_err(backend)?;
        }
        let kind = encoded.map_err(backend)?;
        if kind == FrameKind::Skipped || self.output.is_empty() {
            if params.keyframe {
                return Err(CodecError::InvalidFrame("the encoder skipped a keyframe"));
            }
            return Ok(None);
        }
        Ok(Some(Encoded {
            payload: Bytes::copy_from_slice(&self.output),
            keyframe: kind == FrameKind::Idr,
            width: frame.width(),
            height: frame.height(),
        }))
    }
}

/// One window's OpenH264 decoder.
struct OpenH264Decoder {
    decoder: Decoder,
}

impl VideoDecoder for OpenH264Decoder {
    /// H.264 in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// Decodes a packet and crops the picture to the size it announces,
    /// which the encoder padded to even sides.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError> {
        let (width, height) = (packet.width, packet.height);
        let Some(picture) = self.decoder.decode(packet.payload).map_err(backend)? else {
            return Ok(None);
        };
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (ys, cs) = (picture.y_stride as usize, picture.uv_stride as usize);
        if picture.width < width
            || picture.height < height
            || !yuv::holds(picture.y.len(), ys, w, h)
            || !yuv::holds(picture.u.len(), cs, cw, ch)
            || !yuv::holds(picture.v.len(), cs, cw, ch)
        {
            return Err(CodecError::BadPayload { width, height });
        }
        let pixels = yuv::to_bgra(
            Planes::I420 {
                y: picture.y,
                u: picture.u,
                v: picture.v,
                y_stride: ys,
                uv_stride: cs,
            },
            width,
            height,
        );
        Ok(Some(
            Frame::packed(width, height, pixels)?.with_timestamp(packet.timestamp),
        ))
    }
}
