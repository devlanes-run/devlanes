//! The codecs this machine has: the encoders `host` offers, the decoders
//! `view` offers, and `devlanes-gui codecs`, which `devlanes doctor` runs.
//! Best first, with `raw` last, since every build has it.

use std::sync::Arc;

use devlanes_gui_codec::{DecoderBackend, EncoderBackend, RawBackend, Unavailable};
use devlanes_gui_proto::ready::{CodecProblem, CodecReason};

/// This machine's codecs.
pub struct Codecs {
    /// For `host`: H.264 where there is an encoder, then `raw`.
    pub encoders: Vec<Arc<dyn EncoderBackend>>,
    /// For `view`: H.264 where there is a decoder, then `raw`.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// Why H.264 is missing, when it is.
    pub problems: Vec<CodecProblem>,
    /// The H.264 library in use, such as `OpenH264 2.6.0`.
    pub version: Option<String>,
}

/// Finds this machine's codecs. Blocking: it loads OpenH264 on Linux and
/// enumerates Media Foundation's transforms on Windows, which uses COM, so a
/// caller on a thread whose COM apartment matters runs it on another thread.
pub fn find() -> Codecs {
    let h264 = devlanes_gui_codec::h264();
    let mut encoders = h264.encoders;
    encoders.push(Arc::new(RawBackend));
    let mut decoders = h264.decoders;
    decoders.push(Arc::new(RawBackend));
    Codecs {
        encoders,
        decoders,
        problems: h264.unavailable.map(problem).into_iter().collect(),
        version: h264.version,
    }
}

/// The facts of a missing H.264, for the ready line and `codecs`.
fn problem(unavailable: Unavailable) -> CodecProblem {
    let blank = CodecProblem {
        codec: "h264".to_owned(),
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
        Unavailable::Os { detail } => CodecProblem {
            reason: CodecReason::Unavailable,
            detail,
            ..blank
        },
    }
}

/// What `devlanes-gui codecs` prints: one JSON line with the H.264 library in
/// use, which ends have it in hardware, and why it is missing.
///
/// ```json
/// {"h264":{"version":"OpenH264 2.6.0","encoders":["software"],"decoders":["software"]},"problems":[]}
/// ```
pub fn report(codecs: &Codecs) -> serde_json::Value {
    let kinds = |hardware: Vec<bool>| -> Vec<&'static str> {
        hardware
            .into_iter()
            .map(|hardware| if hardware { "hardware" } else { "software" })
            .collect()
    };
    let h264 = devlanes_gui_proto::CodecId::H264;
    let encoders = codecs
        .encoders
        .iter()
        .map(|e| e.offer())
        .filter(|offer| offer.codec == h264)
        .map(|offer| offer.hardware)
        .collect();
    let decoders = codecs
        .decoders
        .iter()
        .map(|d| d.offer())
        .filter(|offer| offer.codec == h264)
        .map(|offer| offer.hardware)
        .collect();
    serde_json::json!({
        "h264": {
            "version": codecs.version,
            "encoders": kinds(encoders),
            "decoders": kinds(decoders),
        },
        "problems": codecs.problems,
    })
}
