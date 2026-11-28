//! The codecs this machine has: the encoders `host` offers, the decoders
//! `view` offers, and `devlanes-gui codecs`, which `devlanes doctor` runs.
//! H.264 first where it works, then `raw`, which every build has.

use std::sync::Arc;

use devlanes_gui_codec::{DecoderBackend, EncoderBackend, RawBackend, Unavailable};
use devlanes_gui_proto::ready::{CodecProblem, CodecReason, CodecSide};

/// This machine's codecs.
pub struct Codecs {
    /// For `host`: H.264 where this machine encodes it, then `raw`.
    pub encoders: Vec<Arc<dyn EncoderBackend>>,
    /// For `view`: H.264 where this machine decodes it, then `raw`.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// Why H.264 encoding or decoding is missing, a problem per side.
    pub problems: Vec<CodecProblem>,
    /// The H.264 library in use, such as `OpenH264 2.6.0`.
    pub version: Option<String>,
}

impl Codecs {
    /// The problems of one side: a host reports what it cannot encode, a
    /// viewer what it cannot decode.
    pub fn problems_of(&self, side: CodecSide) -> Vec<CodecProblem> {
        self.problems
            .iter()
            .filter(|problem| problem.side == side)
            .cloned()
            .collect()
    }
}

/// Finds this machine's codecs. Blocking: it loads OpenH264 and starts an
/// encoder and a decoder on Linux, and enumerates Media Foundation's
/// transforms on Windows, which uses COM, so a caller on a thread whose COM
/// apartment matters runs it on another thread.
pub fn find() -> Codecs {
    let h264 = devlanes_gui_codec::h264();
    let mut encoders = h264.encoders;
    encoders.push(Arc::new(RawBackend));
    let mut decoders = h264.decoders;
    decoders.push(Arc::new(RawBackend));
    let problems = [
        (CodecSide::Encode, h264.encode),
        (CodecSide::Decode, h264.decode),
    ]
    .into_iter()
    .filter_map(|(side, unavailable)| unavailable.map(|u| problem(side, u)))
    .collect();
    Codecs {
        encoders,
        decoders,
        problems,
        version: h264.version,
    }
}

/// The facts of a missing side of H.264, for the ready line and `codecs`.
fn problem(side: CodecSide, unavailable: Unavailable) -> CodecProblem {
    let blank = CodecProblem {
        codec: "h264".to_owned(),
        side,
        reason: CodecReason::Other,
        library: None,
        found: None,
        supported: None,
        detail: String::new(),
    };
    match unavailable {
        Unavailable::MissingLibrary { library, detail } => CodecProblem {
            reason: CodecReason::MissingLibrary,
            library: Some(library.to_owned()),
            detail,
            ..blank
        },
        Unavailable::UnsupportedVersion {
            library,
            found,
            supported,
        } => CodecProblem {
            reason: CodecReason::UnsupportedVersion,
            library: Some(library.to_owned()),
            found: Some(found),
            supported: Some(supported),
            ..blank
        },
        Unavailable::DecoderTooOld {
            library,
            found,
            supported,
        } => CodecProblem {
            reason: CodecReason::DecoderTooOld,
            library: Some(library.to_owned()),
            found: Some(found),
            supported: Some(supported),
            ..blank
        },
        Unavailable::Os { detail } => CodecProblem {
            reason: CodecReason::Unavailable,
            detail,
            ..blank
        },
    }
}

/// What `devlanes-gui codecs` prints: one JSON line with the H.264 library in
/// use, whether this machine encodes and decodes it and with what kind of
/// encoder and decoder, and a problem per missing side.
///
/// ```json
/// {"h264":{"version":"OpenH264 2.6.0","encode":true,"decode":true,"encoders":["software"],"decoders":["software"]},"problems":[]}
/// ```
pub fn report(codecs: &Codecs) -> serde_json::Value {
    let h264 = devlanes_gui_proto::CodecId::H264;
    let kind = |hardware: bool| if hardware { "hardware" } else { "software" };
    let encoders: Vec<&str> = codecs
        .encoders
        .iter()
        .map(|e| e.offer())
        .filter(|offer| offer.codec == h264)
        .map(|offer| kind(offer.hardware))
        .collect();
    let decoders: Vec<&str> = codecs
        .decoders
        .iter()
        .map(|d| d.offer())
        .filter(|offer| offer.codec == h264)
        .map(|offer| kind(offer.hardware))
        .collect();
    serde_json::json!({
        "h264": {
            "version": codecs.version,
            "encode": !encoders.is_empty(),
            "decode": !decoders.is_empty(),
            "encoders": encoders,
            "decoders": decoders,
        },
        "problems": codecs.problems,
    })
}
