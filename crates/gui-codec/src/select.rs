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
/// The order is fixed, with no user setting: the newer codec first, so H.264
/// beats `raw`. Within a codec the host's encoders are tried in the order it
/// passes them, which for each window follows `EncoderBackend::rank` at the
/// window's size (on Windows Microsoft's software encoder first for windows
/// up to about 2.5 megapixels, since it was measured faster there and has
/// no session cap and no driver to hang, and the hardware one first above),
/// and the viewer decodes with its hardware decoder where it lists one. `None` only when the lists
/// share no codec, which cannot happen between devlanes builds since both
/// have `raw`.
pub fn select(encoders: &[CodecOffer], decoders: &[CodecOffer]) -> Option<Selection> {
    let best_decoder = |codec: CodecId| {
        decoders
            .iter()
            .filter(|offer| offer.codec == codec)
            .map(|offer| offer.hardware)
            .max()
    };
    let mut chosen: Option<(u8, Selection)> = None;
    for offer in encoders {
        let (Some(rank), Some(decoder_hardware)) = (rank(offer.codec), best_decoder(offer.codec))
        else {
            continue;
        };
        // The first encoder listed for a codec wins among its codec's.
        if chosen.is_none_or(|(best, _)| rank > best) {
            chosen = Some((
                rank,
                Selection {
                    codec: offer.codec,
                    encoder_hardware: offer.hardware,
                    decoder_hardware,
                },
            ));
        }
    }
    chosen.map(|(_, selection)| selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An offer of `codec`.
    fn offer(codec: CodecId, hardware: bool) -> CodecOffer {
        CodecOffer { codec, hardware }
    }

    /// The newer codec wins, the host's first encoder of it is the one used,
    /// the viewer's hardware decoder is preferred, and unknown codecs never
    /// win.
    #[test]
    fn the_newer_codec_wins_in_the_host_order_and_unknown_codecs_never() {
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
            // Windows lists its software encoder first.
            (
                &[raw, h264_sw, h264_hw],
                &[raw, h264_sw],
                Some((CodecId::H264, false, false)),
            ),
            (
                &[h264_hw, h264_sw, raw],
                &[raw, h264_sw, h264_hw],
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
