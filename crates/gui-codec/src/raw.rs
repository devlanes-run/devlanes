//! The `raw` codec: each frame is its packed BGRA rows compressed with zstd.
//!
//! Every frame is a keyframe, so there is no state to lose, and every build
//! has it: it is the fallback when the two ends share no other codec, and what
//! screenshots and tests use. It is heavy over a network.

use bytes::Bytes;
use devlanes_gui_proto::{CodecId, CodecOffer, MAX_DIMENSION};

use crate::{
    CodecError, DecoderBackend, EncodeParams, Encoded, EncoderBackend, Frame, Packet, VideoDecoder,
    VideoEncoder,
};

/// zstd's fastest level: screen content still compresses well, and the
/// encoder must keep up with the frame rate on one core.
const LEVEL: i32 = 1;

/// What every raw encoder and decoder offers.
const OFFER: CodecOffer = CodecOffer {
    codec: CodecId::RAW,
    hardware: false,
};

/// Wraps a zstd error.
fn zstd_error(source: std::io::Error) -> CodecError {
    CodecError::Backend {
        codec: "raw",
        source: Box::new(source),
    }
}

/// Encodes frames as compressed BGRA and skips pictures that did not change.
#[derive(Debug, Default)]
pub struct RawEncoder {
    /// The last picture sent, to skip an identical one.
    previous: Option<(u32, u32, Bytes)>,
}

impl RawEncoder {
    /// An encoder that has sent nothing yet.
    pub fn new() -> Self {
        Self::default()
    }
}

impl VideoEncoder for RawEncoder {
    /// The raw codec, in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// Compresses the frame, or returns `None` when it equals the previous one
    /// and no keyframe was asked for. The bitrate and quality are ignored.
    fn encode(
        &mut self,
        frame: &Frame,
        params: &EncodeParams,
    ) -> Result<Option<Encoded>, CodecError> {
        let (width, height) = (frame.width(), frame.height());
        let pixels = frame.to_packed();
        let unchanged = self
            .previous
            .as_ref()
            .is_some_and(|(w, h, before)| (*w, *h) == (width, height) && *before == pixels);
        if unchanged && !params.keyframe {
            return Ok(None);
        }
        let payload = zstd::bulk::compress(&pixels, LEVEL).map_err(zstd_error)?;
        self.previous = Some((width, height, pixels));
        Ok(Some(Encoded {
            payload: Bytes::from(payload),
            keyframe: true,
            width,
            height,
        }))
    }
}

/// Decodes raw frames.
#[derive(Debug, Default)]
pub struct RawDecoder;

impl RawDecoder {
    /// A decoder.
    pub fn new() -> Self {
        Self
    }
}

impl VideoDecoder for RawDecoder {
    /// The raw codec, in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// Decompresses a packet into a packed frame. The size it announces bounds
    /// the memory, so a payload that inflates further is an error, not an
    /// allocation.
    fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Frame>, CodecError> {
        let (width, height) = (packet.width, packet.height);
        let bad = || CodecError::BadPayload { width, height };
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(bad());
        }
        let expected = width as usize * height as usize * 4;
        let pixels = zstd::bulk::decompress(packet.payload, expected).map_err(|_| bad())?;
        if pixels.len() != expected {
            return Err(bad());
        }
        Frame::packed(width, height, pixels).map(Some)
    }
}

/// Creates raw encoders and decoders; every host and viewer registers it.
#[derive(Debug, Default, Clone, Copy)]
pub struct RawBackend;

impl EncoderBackend for RawBackend {
    /// The raw codec, in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// A [`RawEncoder`].
    fn encoder(&self) -> Result<Box<dyn VideoEncoder>, CodecError> {
        Ok(Box::new(RawEncoder::new()))
    }
}

impl DecoderBackend for RawBackend {
    /// The raw codec, in software.
    fn offer(&self) -> CodecOffer {
        OFFER
    }

    /// A [`RawDecoder`].
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, CodecError> {
        Ok(Box::new(RawDecoder::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELTA: EncodeParams = EncodeParams {
        bitrate_bps: 1_000_000,
        quality: 50,
        keyframe: false,
    };

    /// A padded frame whose pixels depend on `seed`.
    fn picture(width: u32, height: u32, seed: u8) -> Frame {
        let stride = width * 4 + 8;
        let data: Vec<u8> = (0..stride * height)
            .map(|i| (i % 251) as u8 ^ seed)
            .collect();
        Frame::new(width, height, stride, data).expect("valid frame")
    }

    /// Decodes an encoder's output with a fresh decoder.
    fn decode(encoded: &Encoded) -> Frame {
        RawDecoder::new()
            .decode(&Packet {
                payload: &encoded.payload,
                width: encoded.width,
                height: encoded.height,
                keyframe: encoded.keyframe,
            })
            .expect("decodes")
            .expect("a frame")
    }

    #[test]
    fn frames_round_trip_and_unchanged_ones_are_skipped_unless_a_keyframe_is_due() {
        let mut encoder = RawEncoder::new();
        let first = picture(32, 16, 0);
        let encoded = encoder
            .encode(&first, &DELTA)
            .expect("encodes")
            .expect("sent");
        let decoded = decode(&encoded);
        assert_eq!(decoded.to_packed(), first.to_packed());
        assert_eq!(decoded.stride(), 32 * 4);

        assert_eq!(
            encoder.encode(&first.clone(), &DELTA).expect("encodes"),
            None
        );
        let keyframe = EncodeParams {
            keyframe: true,
            ..DELTA
        };
        assert!(
            encoder
                .encode(&first, &keyframe)
                .expect("encodes")
                .is_some()
        );

        let changed = picture(32, 16, 1);
        let encoded = encoder
            .encode(&changed, &DELTA)
            .expect("encodes")
            .expect("sent");
        assert_eq!(decode(&encoded).to_packed(), changed.to_packed());
    }

    #[test]
    fn a_size_change_is_sent_even_when_the_bytes_are_the_same() {
        let mut encoder = RawEncoder::new();
        let wide = Frame::packed(4, 2, vec![9; 32]).expect("valid");
        let tall = Frame::packed(2, 4, vec![9; 32]).expect("valid");
        encoder.encode(&wide, &DELTA).expect("encodes");
        let encoded = encoder
            .encode(&tall, &DELTA)
            .expect("encodes")
            .expect("sent");
        let decoded = decode(&encoded);
        assert_eq!((decoded.width(), decoded.height()), (2, 4));
    }

    #[test]
    fn a_payload_that_does_not_match_its_size_is_an_error() {
        let mut encoder = RawEncoder::new();
        let encoded = encoder
            .encode(&picture(8, 8, 0), &DELTA)
            .expect("encodes")
            .expect("sent");
        let mut decoder = RawDecoder::new();
        for (width, height) in [(8, 7), (8, 9), (0, 8), (MAX_DIMENSION + 1, 1)] {
            let packet = Packet {
                payload: &encoded.payload,
                width,
                height,
                keyframe: true,
            };
            assert!(
                matches!(decoder.decode(&packet), Err(CodecError::BadPayload { .. })),
                "{width}x{height}"
            );
        }
        let garbage = Packet {
            payload: b"not zstd",
            width: 8,
            height: 8,
            keyframe: true,
        };
        assert!(decoder.decode(&garbage).is_err());
    }
}
