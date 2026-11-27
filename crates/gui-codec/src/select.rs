//! Picking the codec a window streams in.

use devlanes_gui_proto::{CodecId, CodecOffer};

/// The codec both ends use, and which ends run it in hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The codec.
    pub codec: CodecId,
    /// Whether the host's encoder for it is hardware.
    pub encoder_hardware: bool,
    /// Whether the viewer's decoder for it is hardware.
    pub decoder_hardware: bool,
}

/// How much the selector prefers a codec, newer before older; `None` for codecs
/// it does not know, which are never picked. A new codec gets a rank here.
fn rank(codec: CodecId) -> Option<u8> {
    match codec {
        CodecId::H264 => Some(2),
        CodecId::RAW => Some(1),
        _ => None,
    }
}

/// Picks the best codec the host can encode and one viewer can decode.
///
/// The host runs it per viewer. Viewers of a window that get the same codec
/// share one encoder and one pacer; a viewer that gets another codec has an
/// encoder and a pacer of its own for that window.
///
/// The order is fixed, with no user setting: more ends in hardware first, then
/// the newer codec, so hardware H.264 beats software H.264, which beats `raw`.
/// For each codec, an end that lists both a hardware and a software
/// implementation counts as hardware. `None` only when the lists share no
/// codec, which cannot happen between devlanes builds since both have `raw`.
pub fn select(encoders: &[CodecOffer], decoders: &[CodecOffer]) -> Option<Selection> {
    let best = |offers: &[CodecOffer], codec: CodecId| {
        offers
            .iter()
            .filter(|offer| offer.codec == codec)
            .map(|offer| offer.hardware)
            .max()
    };
    encoders
        .iter()
        .filter_map(|offer| {
            let rank = rank(offer.codec)?;
            let encoder_hardware = best(encoders, offer.codec)?;
            let decoder_hardware = best(decoders, offer.codec)?;
            let hardware_ends = u8::from(encoder_hardware) + u8::from(decoder_hardware);
            Some((
                (hardware_ends, rank),
                Selection {
                    codec: offer.codec,
                    encoder_hardware,
                    decoder_hardware,
                },
            ))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, selection)| selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An offer of `codec`.
    fn offer(codec: CodecId, hardware: bool) -> CodecOffer {
        CodecOffer { codec, hardware }
    }

    #[test]
    fn hardware_wins_then_the_newer_codec_and_unknown_codecs_never() {
        let raw = offer(CodecId::RAW, false);
        let h264_sw = offer(CodecId::H264, false);
        let h264_hw = offer(CodecId::H264, true);
        let future = offer(CodecId(77), true);
        type Case<'a> = (
            &'a [CodecOffer],
            &'a [CodecOffer],
            Option<(CodecId, bool, bool)>,
        );
        let cases: &[Case<'_>] = &[
            (
                &[raw, h264_sw],
                &[raw, h264_sw],
                Some((CodecId::H264, false, false)),
            ),
            (
                &[raw, h264_sw, h264_hw],
                &[raw, h264_sw],
                Some((CodecId::H264, true, false)),
            ),
            (
                &[raw, h264_hw],
                &[h264_hw, raw],
                Some((CodecId::H264, true, true)),
            ),
            // A Linux viewer without OpenH264 falls back to raw.
            (&[raw, h264_hw], &[raw], Some((CodecId::RAW, false, false))),
            (
                &[future, raw],
                &[future, raw],
                Some((CodecId::RAW, false, false)),
            ),
            (&[h264_sw], &[raw], None),
            (&[], &[raw], None),
        ];
        for (encoders, decoders, expected) in cases {
            let got = select(encoders, decoders)
                .map(|s| (s.codec, s.encoder_hardware, s.decoder_hardware));
            assert_eq!(got, *expected, "{encoders:?} to {decoders:?}");
        }
    }
}
