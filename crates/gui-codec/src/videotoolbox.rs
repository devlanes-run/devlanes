//! H.264 decoding on macOS through VideoToolbox
//! (`devlanes_sys::videotoolbox`), for the viewer. The macOS encoder comes
//! with the macOS host.

use devlanes_gui_proto::{CodecId, CodecOffer};
use devlanes_sys::videotoolbox::Decoder;

use crate::yuv::{self, Planes};
use crate::{CodecError, DecoderBackend, Frame, Packet, VideoDecoder};

/// What VideoToolbox decoders offer: Apple's decoder runs in hardware on
/// every Mac devlanes supports.
const OFFER: CodecOffer = CodecOffer {
    codec: CodecId::H264,
    hardware: true,
};

/// The VideoToolbox decoder backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct VideoToolbox;

impl DecoderBackend for VideoToolbox {
    /// H.264 in hardware.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// A decoder; its session starts on the stream's first parameter sets.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(Decode(Decoder::new())))
    }
}

/// One window's decoder.
struct Decode(Decoder);

impl VideoDecoder for Decode {
    /// H.264 in hardware.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// Decodes a packet and crops the picture to the size it announces.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError> {
        let (width, height) = (packet.width, packet.height);
        if packet.payload.is_empty() {
            return Err(CodecError::BadPayload { width, height });
        }
        let picture = self.0.decode(packet.payload).map_err(|error| match error {
            // A picture larger than any frame message announces.
            devlanes_sys::videotoolbox::Error::TooLarge { .. } => {
                CodecError::BadPayload { width, height }
            }
            other => CodecError::Backend {
                codec: "h264",
                source: Box::new(other),
            },
        })?;
        let Some(picture) = picture else {
            return Ok(None);
        };
        let (w, h) = (width as usize, height as usize);
        if picture.width < width
            || picture.height < height
            || !yuv::holds(picture.y.len(), picture.y_stride, w, h)
            || !yuv::holds(
                picture.uv.len(),
                picture.uv_stride,
                2 * w.div_ceil(2),
                h.div_ceil(2),
            )
        {
            return Err(CodecError::BadPayload { width, height });
        }
        let pixels = yuv::to_bgra(
            Planes::Nv12 {
                y: &picture.y,
                uv: &picture.uv,
                y_stride: picture.y_stride,
                uv_stride: picture.uv_stride,
            },
            width,
            height,
        );
        Ok(Some(
            Frame::packed(width, height, pixels)?.with_timestamp(packet.timestamp),
        ))
    }
}
