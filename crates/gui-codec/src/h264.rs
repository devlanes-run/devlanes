//! The H.264 backends this machine has: OpenH264 on Linux, Media Foundation
//! on Windows, the VideoToolbox decoder on macOS. `devlanes-gui` registers
//! what [`h264`] finds, and reports what is missing, separately for encoding
//! (what a GUI host needs) and decoding (what a viewer needs).

use std::sync::Arc;

#[cfg(any(target_os = "linux", windows))]
use crate::EncodeParams;
use crate::{DecoderBackend, EncoderBackend};

/// The frame rate a frame interval allows, within what rate control plans for.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn frame_rate(params: &EncodeParams) -> f32 {
    let seconds = params.frame_interval.as_secs_f32();
    if seconds > 0.0 {
        (1.0 / seconds).clamp(1.0, 60.0)
    } else {
        30.0
    }
}

/// The quantizer (0 best to 51 worst) for the pacer's quality from 0 to
/// 100: 44, blocky but small, at 0, down to 18 at 100.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn quantizer(quality: u8) -> u8 {
    44 - (u32::from(quality.min(100)) * 26 / 100) as u8
}

/// Whether a picture of `width` by `height` fits H.264 level 5.2, the
/// highest the software encoders here support: at most 36,864 macroblocks
/// (4096x2304) and sides of at most 4096.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn level_5_2(width: u32, height: u32) -> bool {
    let blocks = |side: u32| u64::from(side.div_ceil(16));
    width <= 4096 && height <= 4096 && blocks(width) * blocks(height) <= 36_864
}

/// Why H.264 encoding or decoding is not available on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The codec library is not installed (OpenH264 on Linux).
    MissingLibrary {
        /// The library, such as `OpenH264`.
        library: &'static str,
        /// The loader's message.
        detail: String,
    },
    /// The installed library is a version devlanes cannot use at all.
    UnsupportedVersion {
        /// The library.
        library: &'static str,
        /// Its version.
        found: String,
        /// The versions devlanes uses, such as `2.2 to 2.7`.
        supported: String,
    },
    /// The installed library encodes, but its decoder has a known security
    /// flaw (OpenH264 before 2.6), so it does not decode.
    DecoderTooOld {
        /// The library.
        library: &'static str,
        /// Its version.
        found: String,
        /// The versions whose decoder is used, such as `2.6 or later`.
        supported: String,
    },
    /// The OS has no H.264 codec, or the one found does not start.
    Os {
        /// What failed.
        detail: String,
    },
}

/// H.264 on this machine: the backends to register, and why a side is
/// missing.
pub struct H264 {
    /// Encoders in the order to try them.
    pub encoders: Vec<Arc<dyn EncoderBackend>>,
    /// Decoders, best first.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// Why there is no encoder, when there is none.
    pub encode: Option<Unavailable>,
    /// Why there is no decoder, when there is none.
    pub decode: Option<Unavailable>,
    /// The library and version in use, such as `OpenH264 2.6.0`, for logs.
    pub version: Option<String>,
}

/// Finds this machine's H.264 backends, and on Linux starts and drops an
/// encoder and a decoder, so a library that loads but does not work counts
/// as unavailable. Blocking: it loads libraries and enumerates OS transforms.
pub fn h264() -> H264 {
    os()
}

#[cfg(target_os = "linux")]
fn os() -> H264 {
    use devlanes_sys::openh264::{
        DECODER_SUPPORTED, Decoder, Encoder, EncoderConfig, Error, LoadError, SUPPORTED,
    };

    let library = match devlanes_sys::openh264::Library::load() {
        Ok(library) => library,
        Err(error) => {
            let unavailable = match error {
                LoadError::Unsupported { found, .. } => Unavailable::UnsupportedVersion {
                    library: "OpenH264",
                    found: found.to_string(),
                    supported: SUPPORTED.to_owned(),
                },
                missing @ LoadError::Missing { .. } => Unavailable::MissingLibrary {
                    library: "OpenH264",
                    detail: missing.to_string(),
                },
                other => Unavailable::Os {
                    detail: other.to_string(),
                },
            };
            return H264 {
                encoders: Vec::new(),
                decoders: Vec::new(),
                encode: Some(unavailable.clone()),
                decode: Some(unavailable),
                version: None,
            };
        }
    };
    let backend = Arc::new(crate::OpenH264::new(library.clone()));
    let probe = EncoderConfig {
        width: 64,
        height: 64,
        bitrate_bps: 500_000,
        frame_rate: 30.0,
    };
    let encode = Encoder::new(&library, probe)
        .err()
        .map(|error| Unavailable::Os {
            detail: error.to_string(),
        });
    let decode = Decoder::new(&library).err().map(|error| match error {
        Error::DecoderTooOld(found) => Unavailable::DecoderTooOld {
            library: "OpenH264",
            found: found.to_string(),
            supported: DECODER_SUPPORTED.to_owned(),
        },
        other => Unavailable::Os {
            detail: other.to_string(),
        },
    });
    let encoders: Vec<Arc<dyn EncoderBackend>> = match encode {
        None => vec![backend.clone()],
        Some(_) => Vec::new(),
    };
    let decoders: Vec<Arc<dyn DecoderBackend>> = match decode {
        None => vec![backend],
        Some(_) => Vec::new(),
    };
    H264 {
        version: (!encoders.is_empty() || !decoders.is_empty())
            .then(|| format!("OpenH264 {}", library.version())),
        encoders,
        decoders,
        encode,
        decode,
    }
}

#[cfg(windows)]
fn os() -> H264 {
    crate::media_foundation::backends()
}

#[cfg(target_os = "macos")]
fn os() -> H264 {
    H264 {
        encoders: Vec::new(),
        decoders: vec![Arc::new(crate::VideoToolbox)],
        // macOS has no GUI host in devlanes, so nothing needs an encoder.
        encode: None,
        decode: None,
        version: Some("VideoToolbox".to_owned()),
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn os() -> H264 {
    let none = Unavailable::Os {
        detail: "this OS has no H.264 backend in devlanes".to_owned(),
    };
    H264 {
        encoders: Vec::new(),
        decoders: Vec::new(),
        encode: Some(none.clone()),
        decode: Some(none),
        version: None,
    }
}
