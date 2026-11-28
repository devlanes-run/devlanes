//! H.264 on Windows through Media Foundation (`devlanes_sys_windows::
//! media_foundation`): Microsoft's software encoder and a hardware encoder
//! where the GPU has one, each the other's fallback, and Microsoft's
//! decoder.
//!
//! Which encoder a window tries first depends on its size
//! ([`LARGE_WINDOW_PIXELS`]). Up to it the software encoder comes first: it
//! was faster where it was measured, has no session cap and no driver to
//! hang. Above it the hardware encoder does, since software then costs
//! several times the CPU for the same time per frame, and large windows are
//! few, so the session cap rarely matters. The quantizer of every frame, a
//! refinement's included, comes from the quality the pacer asks for.

use std::sync::Arc;

use bytes::Bytes;
use devlanes_gui_proto::{CodecId, CodecOffer};
use devlanes_sys_windows::media_foundation::{self as mf, Decoder, Encoder, EncoderConfig};

use crate::h264::{H264, Unavailable, frame_rate, level_5_2, quantizer, refine_qp};
use crate::yuv::{self, Planes};
use crate::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, VideoDecoder,
    VideoEncoder,
};

/// The window size, in pixels, above which the hardware encoder comes first.
const LARGE_WINDOW_PIXELS: u64 = 2_500_000;

/// Maps a Media Foundation error.
fn codec_error(error: mf::Error) -> CodecError {
    match error {
        mf::Error::Unavailable(reason) => CodecError::Unavailable {
            codec: "h264",
            reason,
        },
        other => CodecError::Backend {
            codec: "h264",
            source: Box::new(other),
        },
    }
}

/// Finds the Media Foundation encoders, software first, and the decoder.
/// Blocking.
pub(crate) fn backends() -> H264 {
    let mut encoders: Vec<Arc<dyn EncoderBackend>> = Vec::new();
    let mut problems = Vec::new();
    match mf::has_software_encoder() {
        Ok(true) => encoders.push(Arc::new(MediaFoundationEncoder { hardware: false })),
        Ok(false) => problems.push("no software H.264 encoder transform".to_owned()),
        Err(error) => problems.push(error.to_string()),
    }
    match mf::has_hardware_encoder() {
        Ok(true) => encoders.push(Arc::new(MediaFoundationEncoder { hardware: true })),
        Ok(false) => {}
        Err(error) => problems.push(error.to_string()),
    }
    let encode = encoders.is_empty().then(|| Unavailable::Os {
        detail: problems.join("; "),
    });
    let (decoders, decode): (Vec<Arc<dyn DecoderBackend>>, _) = match Decoder::new() {
        Ok(_) => (vec![Arc::new(MediaFoundationDecoder)], None),
        Err(error) => (
            Vec::new(),
            Some(Unavailable::Os {
                detail: error.to_string(),
            }),
        ),
    };
    H264 {
        version: (!encoders.is_empty() || !decoders.is_empty())
            .then(|| "Media Foundation".to_owned()),
        encoders,
        decoders,
        encode,
        decode,
    }
}

/// Media Foundation's H.264 encoders of one kind, hardware or software.
#[derive(Debug, Clone, Copy)]
pub struct MediaFoundationEncoder {
    /// Whether this backend uses the GPU's encoder.
    pub hardware: bool,
}

impl EncoderBackend for MediaFoundationEncoder {
    /// H.264, in hardware or not.
    fn offer(&self) -> CodecOffer {
        CodecOffer {
            codec: CodecId::H264,
            hardware: self.hardware,
        }
    }

    /// Microsoft's software encoder documents 4096x2304 at most; GPU
    /// encoders (NVENC among them) take sides of up to 4096 for H.264.
    /// Both within level 5.2.
    fn fits(&self, width: u32, height: u32) -> bool {
        match self.hardware {
            true => level_5_2(width, height),
            false => width <= 4096 && height <= 2304,
        }
    }

    /// The software encoder first up to 2.5 megapixels, the hardware one
    /// above.
    fn rank(&self, width: u32, height: u32) -> u8 {
        let large = u64::from(width) * u64::from(height) > LARGE_WINDOW_PIXELS;
        u8::from(large == self.hardware)
    }

    /// An encoder that starts its transform on the first frame.
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Ok(Box::new(Stream {
            hardware: self.hardware,
            encoder: None,
            stream_qp: None,
            nv12: Vec::new(),
            output: Vec::new(),
        }))
    }
}

/// One window's Media Foundation stream.
struct Stream {
    hardware: bool,
    /// Started on the first frame and again when the size changes.
    encoder: Option<Encoder>,
    /// The quantizer of the last frame that was not a refinement.
    stream_qp: Option<u8>,
    nv12: Vec<u8>,
    output: Vec<u8>,
}

impl VideoEncoder for Stream {
    /// H.264, in hardware or not.
    fn offer(&self) -> CodecOffer {
        CodecOffer {
            codec: CodecId::H264,
            hardware: self.hardware,
        }
    }

    /// Encodes `frame` padded to even sides; a new size starts a new
    /// transform, whose first frame is a keyframe.
    fn encode(
        &mut self,
        frame: &Frame,
        params: &EncodeParams,
    ) -> Result<Option<Encoded>, CodecError> {
        let (width, height) = (yuv::even(frame.width()), yuv::even(frame.height()));
        let (w, h) = (width as usize, height as usize);
        self.nv12.resize(w * h * 3 / 2, 0);
        yuv::to_nv12(frame, &mut self.nv12, w, w * h);
        let bitrate = params.bitrate_bps.max(1);
        // A refinement's quality is above the stream's, so its quantizer is
        // finer, within `refine_qp`'s bound.
        let qp = match (params.refine, self.stream_qp) {
            (true, Some(stream)) => refine_qp(params.quality, stream),
            _ => quantizer(params.quality),
        };
        if !params.refine {
            self.stream_qp = Some(qp);
        }
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
                frame_rate: frame_rate(params).round() as u32,
                qp,
            };
            self.encoder = Some(Encoder::new(config, self.hardware).map_err(codec_error)?);
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Err(CodecError::InvalidFrame("no encoder"));
        };
        // Only a transform without a quantizer per frame follows the bitrate.
        if !encoder.constant_qp() && encoder.config().bitrate_bps != bitrate {
            encoder.set_bitrate(bitrate).map_err(codec_error)?;
        }
        let keyframe = encoder
            .encode(
                &self.nv12,
                frame.timestamp(),
                params.frame_interval,
                params.keyframe && !fresh,
                qp,
                &mut self.output,
            )
            .map_err(codec_error)?;
        let Some(keyframe) = keyframe else {
            if params.keyframe || fresh {
                return Err(CodecError::InvalidFrame("the encoder gave no keyframe"));
            }
            return Ok(None);
        };
        Ok(Some(Encoded {
            payload: Bytes::copy_from_slice(&self.output),
            keyframe,
            width: frame.width(),
            height: frame.height(),
        }))
    }
}

/// Microsoft's H.264 decoder.
#[derive(Debug, Clone, Copy)]
pub struct MediaFoundationDecoder;

impl DecoderBackend for MediaFoundationDecoder {
    /// H.264 in software: the decoder runs without a Direct3D device.
    fn offer(&self) -> CodecOffer {
        CodecOffer {
            codec: CodecId::H264,
            hardware: false,
        }
    }

    /// A decoder.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(Decode(Decoder::new().map_err(codec_error)?)))
    }
}

/// One window's decoder.
struct Decode(Decoder);

impl VideoDecoder for Decode {
    /// H.264 in software.
    fn offer(&self) -> CodecOffer {
        MediaFoundationDecoder.offer()
    }

    /// Decodes a packet and crops the picture to the size it announces.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError> {
        let (width, height) = (packet.width, packet.height);
        let bad = || CodecError::BadPayload { width, height };
        if packet.payload.is_empty() {
            return Err(bad());
        }
        let decoded =
            self.0
                .decode(packet.payload, packet.timestamp)
                .map_err(|error| match error {
                    // A picture larger than any frame message announces.
                    mf::Error::TooLarge { .. } => bad(),
                    other => codec_error(other),
                })?;
        let Some(picture) = decoded else {
            return Ok(None);
        };
        let stride = picture.stride as usize;
        let luma = stride * picture.height as usize;
        let chroma = picture.data.len().checked_sub(luma).ok_or_else(bad)?;
        let (w, h) = (width as usize, height as usize);
        if picture.width < width
            || picture.height < height
            || !yuv::holds(luma, stride, w, h)
            || !yuv::holds(chroma, stride, 2 * w.div_ceil(2), h.div_ceil(2))
        {
            return Err(bad());
        }
        let (y, uv) = picture.data.split_at(luma);
        let pixels = yuv::to_bgra(
            Planes::Nv12 {
                y,
                uv,
                y_stride: stride,
                uv_stride: stride,
            },
            width,
            height,
        );
        Ok(Some(
            Frame::packed(width, height, pixels)?.with_timestamp(packet.timestamp),
        ))
    }
}
