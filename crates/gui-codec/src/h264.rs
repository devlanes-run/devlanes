//! The H.264 backends this machine has: OpenH264 on Linux, Media Foundation
//! on Windows, the VideoToolbox decoder on macOS. `devlanes-gui` registers
//! what [`h264`] finds, and reports what is missing.

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

/// Why H.264 is not available on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The codec library is not installed (OpenH264 on Linux).
    MissingLibrary {
        /// The library, such as `OpenH264`.
        library: &'static str,
        /// The loader's message.
        detail: String,
    },
    /// The installed library is a version devlanes cannot use.
    UnsupportedVersion {
        /// The library.
        library: &'static str,
        /// Its version.
        found: String,
        /// The versions devlanes uses, such as `2.2 to 2.7`.
        supported: String,
    },
    /// The OS has no H.264 transform, or it did not start.
    Os {
        /// What failed.
        detail: String,
    },
}

/// H.264 on this machine: the backends to register, and why a part is missing.
pub struct H264 {
    /// Encoders, best first (hardware before software).
    pub encoders: Vec<Arc<dyn EncoderBackend>>,
    /// Decoders, best first.
    pub decoders: Vec<Arc<dyn DecoderBackend>>,
    /// Why there are none, when there are none.
    pub unavailable: Option<Unavailable>,
    /// The library and version in use, such as `OpenH264 2.6.0`, for logs.
    pub version: Option<String>,
}

/// Finds this machine's H.264 backends. Blocking: it loads libraries and
/// enumerates OS transforms.
pub fn h264() -> H264 {
    os()
}

#[cfg(target_os = "linux")]
fn os() -> H264 {
    use devlanes_sys::openh264::{LoadError, NEWEST, OLDEST};

    match crate::OpenH264::load() {
        Ok(backend) => {
            let version = format!("OpenH264 {}", backend.version());
            let backend = Arc::new(backend);
            H264 {
                encoders: vec![backend.clone()],
                decoders: vec![backend],
                unavailable: None,
                version: Some(version),
            }
        }
        Err(error) => {
            let unavailable = match error {
                LoadError::Unsupported { found, .. } => Unavailable::UnsupportedVersion {
                    library: "OpenH264",
                    found: found.to_string(),
                    supported: format!("{OLDEST} to {}.{}", NEWEST.major, NEWEST.minor),
                },
                other => Unavailable::MissingLibrary {
                    library: "OpenH264",
                    detail: other.to_string(),
                },
            };
            H264 {
                encoders: Vec::new(),
                decoders: Vec::new(),
                unavailable: Some(unavailable),
                version: None,
            }
        }
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
        unavailable: None,
        version: Some("VideoToolbox".to_owned()),
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn os() -> H264 {
    H264 {
        encoders: Vec::new(),
        decoders: Vec::new(),
        unavailable: Some(Unavailable::Os {
            detail: "this OS has no H.264 backend in devlanes".to_owned(),
        }),
        version: None,
    }
}
