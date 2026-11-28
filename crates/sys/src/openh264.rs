//! OpenH264, loaded at run time from the library the user installed, for the
//! Linux GUI host and viewer, behind the `openh264` feature, which only
//! `devlanes-gui` enables. Nothing links it: a machine without it runs
//! every other part of devlanes, and the GUI falls back to the `raw` codec.
//!
//! The bindings are written here rather than taken from the `openh264` crate:
//! its loader accepts only Cisco's own 2.6 and 2.7 binaries (by checksum), and
//! its structures have the 2.6 layout, while distributions ship their own
//! builds of 2.2 to 2.7. Within 2.x the API grew only at the end of
//! structures, except `SLayerBSInfo`, which gained a field in 2.6 (the
//! soname went from 7 to 8): the output of an encode is read with the layout
//! of the loaded version, and versions outside 2.2 to 2.7 are refused, since
//! their layouts are not known.
//!
//! The decoder needs 2.6 or 2.7 ([`DECODER_OLDEST`]): every release before
//! 2.6.0 has a heap overflow in its decoder (CVE-2025-27091), and a viewer
//! decodes what a GUI host on another machine sends. The encoder only
//! encodes the host's own pixels and accepts the whole range.
//!
//! The library is found through the dynamic loader's own search path
//! (`LD_LIBRARY_PATH`, the `ld.so` cache), the same one that decides what any
//! program of the user loads; only the fixed names of [`LIBRARY_NAMES`] are
//! tried.

use std::ffi::{OsStr, c_int, c_long, c_uint, c_ushort, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

/// The names tried by [`Library::load`], newest soname first: 8 is 2.6 and
/// later, 7 is 2.3.1 to 2.5, 6 is 2.2 and 2.3.0. The plain name is what a
/// development package adds.
pub const LIBRARY_NAMES: [&str; 4] = [
    "libopenh264.so.8",
    "libopenh264.so.7",
    "libopenh264.so.6",
    "libopenh264.so",
];

/// The oldest version whose structures these bindings know.
pub const OLDEST: Version = Version {
    major: 2,
    minor: 2,
    revision: 0,
};

/// The newest minor version whose structures these bindings know; any
/// revision of it is accepted.
pub const NEWEST: Version = Version {
    major: 2,
    minor: 7,
    revision: 0,
};

/// The oldest version whose decoder is used: earlier ones have the decoder
/// heap overflow of CVE-2025-27091.
pub const DECODER_OLDEST: Version = Version {
    major: 2,
    minor: 6,
    revision: 0,
};

/// The versions the encoder takes, for messages.
pub const SUPPORTED: &str = "2.2 to 2.7";

/// The versions the decoder takes, for messages: from [`DECODER_OLDEST`] to
/// [`NEWEST`], since a later release is refused on both sides.
pub const DECODER_SUPPORTED: &str = "2.6 to 2.7";

/// The largest picture side the decoder returns, the GUI protocol's limit.
pub const MAX_SIDE: u32 = 8192;

/// An OpenH264 release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// The major number.
    pub major: u32,
    /// The minor number.
    pub minor: u32,
    /// The revision.
    pub revision: u32,
}

impl std::fmt::Display for Version {
    /// `2.6.0`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.revision)
    }
}

/// Why OpenH264 could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// None of the names loads: OpenH264 is not installed.
    #[error("OpenH264 is not installed: none of {} loads ({detail})", .tried.join(", "))]
    Missing {
        /// The names or paths tried.
        tried: Vec<String>,
        /// The loader's message for the last one.
        detail: String,
    },
    /// A library loaded but lacks OpenH264's functions.
    #[error("{path} is not OpenH264: {detail}")]
    NotOpenH264 {
        /// The name or path that loaded.
        path: String,
        /// The missing function.
        detail: String,
    },
    /// OpenH264 of a version whose structures these bindings do not know.
    #[error("OpenH264 {found} at {path} is not supported; devlanes supports {SUPPORTED}")]
    Unsupported {
        /// The name or path that loaded.
        path: String,
        /// Its version.
        found: Version,
    },
}

/// Why an OpenH264 call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A call returned an error code.
    #[error("OpenH264 {call} failed with code {code}")]
    Call {
        /// The call.
        call: &'static str,
        /// What it returned.
        code: i64,
    },
    /// A picture whose planes do not hold its size.
    #[error("invalid picture: {0}")]
    Picture(&'static str),
    /// A decoded picture with a side above [`MAX_SIDE`].
    #[error("the decoded picture is {width}x{height}, larger than {MAX_SIDE}")]
    TooLarge {
        /// Its width.
        width: u32,
        /// Its height.
        height: u32,
    },
    /// A decoder asked of a library older than [`DECODER_OLDEST`].
    #[error(
        "OpenH264 {0}'s decoder is not used: it needs {DECODER_SUPPORTED}, since older versions have a decoder heap overflow (CVE-2025-27091)"
    )]
    DecoderTooOld(Version),
}

/// Which `SLayerBSInfo` the library writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// 2.2 to 2.5.
    Before26,
    /// 2.6 and 2.7, with per-layer PSNR values.
    From26,
}

/// The layout of `version`, or `None` when these bindings do not know it.
fn layout(version: Version) -> Option<Layout> {
    if version < OLDEST || version.major != NEWEST.major || version.minor > NEWEST.minor {
        return None;
    }
    Some(if version.minor >= 6 {
        Layout::From26
    } else {
        Layout::Before26
    })
}

/// Whether these bindings can drive OpenH264 `version`.
pub fn supported(version: Version) -> bool {
    layout(version).is_some()
}

// The C structures of `codec_app_def.h` and `codec_def.h` as of 2.7. Fields
// added after 2.2 sit at the ends, where an older library does not read them.

const MAX_SPATIAL_LAYER_NUM: usize = 4;
const MAX_LAYER_NUM_OF_FRAME: usize = 128;
const MAX_SLICES_NUM_TMP: usize = 35;

const VIDEO_FORMAT_I420: c_int = 23;
const SCREEN_CONTENT_REAL_TIME: c_int = 1;
const RC_BITRATE_MODE: c_int = 1;
const RC_OFF_MODE: c_int = -1;
const PRO_BASELINE: c_int = 66;
const CONSTANT_ID: c_int = 0;
const SPATIAL_LAYER_ALL: c_int = 4;
const ERROR_CON_DISABLE: c_int = 0;
const VIDEO_BITSTREAM_AVC: c_int = 0;
const WELS_LOG_QUIET: c_int = 0;

const ENCODER_OPTION_SVC_ENCODE_PARAM_EXT: c_int = 3;
const ENCODER_OPTION_FRAME_RATE: c_int = 4;
const ENCODER_OPTION_BITRATE: c_int = 5;
const ENCODER_OPTION_RC_MODE: c_int = 8;
const ENCODER_OPTION_TRACE_LEVEL: c_int = 25;
const ENCODER_OPTION_GET_STATISTICS: c_int = 28;
const DECODER_OPTION_TRACE_LEVEL: c_int = 9;

const VIDEO_FRAME_TYPE_IDR: c_int = 1;
const VIDEO_FRAME_TYPE_I: c_int = 2;
const VIDEO_FRAME_TYPE_SKIP: c_int = 4;

#[repr(C)]
struct Version4 {
    major: c_uint,
    minor: c_uint,
    revision: c_uint,
    reserved: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SliceArgument {
    slice_mode: c_int,
    slice_num: c_uint,
    slice_mb_num: [c_uint; MAX_SLICES_NUM_TMP],
    slice_size_constraint: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SpatialLayerConfig {
    video_width: c_int,
    video_height: c_int,
    frame_rate: f32,
    spatial_bitrate: c_int,
    max_spatial_bitrate: c_int,
    profile_idc: c_int,
    level_idc: c_int,
    d_layer_qp: c_int,
    slice_argument: SliceArgument,
    video_signal_type_present: bool,
    video_format: u8,
    full_range: bool,
    color_description_present: bool,
    color_primaries: u8,
    transfer_characteristics: u8,
    color_matrix: u8,
    aspect_ratio_present: bool,
    aspect_ratio: c_int,
    aspect_ratio_ext_width: c_ushort,
    aspect_ratio_ext_height: c_ushort,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EncParamExt {
    usage_type: c_int,
    pic_width: c_int,
    pic_height: c_int,
    target_bitrate: c_int,
    rc_mode: c_int,
    max_frame_rate: f32,
    temporal_layer_num: c_int,
    spatial_layer_num: c_int,
    spatial_layers: [SpatialLayerConfig; MAX_SPATIAL_LAYER_NUM],
    complexity_mode: c_int,
    intra_period: c_uint,
    num_ref_frame: c_int,
    sps_pps_id_strategy: c_int,
    prefix_nal_adding_ctrl: bool,
    enable_ssei: bool,
    simulcast_avc: bool,
    padding_flag: c_int,
    entropy_coding_mode_flag: c_int,
    enable_frame_skip: bool,
    max_bitrate: c_int,
    max_qp: c_int,
    min_qp: c_int,
    max_nal_size: c_uint,
    enable_long_term_reference: bool,
    ltr_ref_num: c_int,
    ltr_mark_period: c_uint,
    multiple_thread_idc: c_ushort,
    use_load_balancing: bool,
    loop_filter_disable_idc: c_int,
    loop_filter_alpha_c0_offset: c_int,
    loop_filter_beta_offset: c_int,
    enable_denoise: bool,
    enable_background_detection: bool,
    enable_adaptive_quant: bool,
    enable_frame_cropping_flag: bool,
    enable_scene_change_detect: bool,
    is_lossless_link: bool,
    // 2.3 and later.
    fix_rc_over_shoot: bool,
    idr_bitrate_ratio: c_int,
    // 2.6 and later.
    psnr_y: bool,
    psnr_u: bool,
    psnr_v: bool,
}

#[repr(C)]
struct SourcePicture {
    color_format: c_int,
    stride: [c_int; 4],
    data: [*mut u8; 4],
    pic_width: c_int,
    pic_height: c_int,
    time_stamp: i64,
    // 2.6 and later.
    psnr_y: bool,
    psnr_u: bool,
    psnr_v: bool,
}

/// `SLayerBSInfo` before 2.6.
#[repr(C)]
#[derive(Clone, Copy)]
struct LayerBsInfoOld {
    temporal_id: u8,
    spatial_id: u8,
    quality_id: u8,
    frame_type: c_int,
    layer_type: u8,
    sub_seq_id: c_int,
    nal_count: c_int,
    nal_length_in_byte: *mut c_int,
    bs_buf: *mut u8,
}

/// `SLayerBSInfo` from 2.6 on.
#[repr(C)]
#[derive(Clone, Copy)]
struct LayerBsInfoNew {
    temporal_id: u8,
    spatial_id: u8,
    quality_id: u8,
    frame_type: c_int,
    layer_type: u8,
    sub_seq_id: c_int,
    nal_count: c_int,
    nal_length_in_byte: *mut c_int,
    bs_buf: *mut u8,
    psnr: [f32; 3],
}

#[repr(C)]
struct FrameBsInfo<L> {
    layer_num: c_int,
    layer_info: [L; MAX_LAYER_NUM_OF_FRAME],
    frame_type: c_int,
    frame_size_in_bytes: c_int,
    time_stamp: i64,
}

/// `SEncoderStatistics`, unchanged from 2.2 to 2.7.
#[repr(C)]
struct EncoderStatistics {
    width: c_uint,
    height: c_uint,
    average_frame_speed_in_ms: f32,
    average_frame_rate: f32,
    latest_frame_rate: f32,
    bit_rate: c_uint,
    average_frame_qp: c_uint,
    input_frame_count: c_uint,
    skipped_frame_count: c_uint,
    resolution_change_times: c_uint,
    idr_req_num: c_uint,
    idr_sent_num: c_uint,
    ltr_sent_num: c_uint,
    statistics_ts: i64,
    total_encoded_bytes: std::ffi::c_ulong,
    last_statistics_bytes: std::ffi::c_ulong,
    last_statistics_frame_count: std::ffi::c_ulong,
}

#[repr(C)]
struct BitrateInfo {
    layer: c_int,
    bitrate: c_int,
}

#[repr(C)]
struct VideoProperty {
    size: c_uint,
    bitstream_type: c_int,
}

#[repr(C)]
struct DecodingParam {
    file_name_restructed: *mut std::ffi::c_char,
    cpu_load: c_uint,
    target_dq_layer: u8,
    ec_active_idc: c_int,
    parse_only: bool,
    video_property: VideoProperty,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SysMemBuffer {
    width: c_int,
    height: c_int,
    format: c_int,
    stride: [c_int; 2],
}

#[repr(C)]
struct BufferInfo {
    buffer_status: c_int,
    in_bs_time_stamp: u64,
    out_yuv_time_stamp: u64,
    system_buffer: SysMemBuffer,
    dst: [*mut u8; 3],
}

/// An encoder object: a pointer to its table of virtual functions, which is
/// how C sees the C++ `ISVCEncoder`.
type EncoderObject = *const EncoderVtable;

/// The `ISVCEncoder` virtual functions in declaration order. The C++
/// `ForceIntraFrame` takes the layer as a second argument, which the C header
/// leaves out; it is passed here, as -1 for every layer.
#[repr(C)]
struct EncoderVtable {
    initialize: unsafe extern "C" fn(*mut EncoderObject, *const c_void) -> c_int,
    initialize_ext: unsafe extern "C" fn(*mut EncoderObject, *const EncParamExt) -> c_int,
    get_default_params: unsafe extern "C" fn(*mut EncoderObject, *mut EncParamExt) -> c_int,
    uninitialize: unsafe extern "C" fn(*mut EncoderObject) -> c_int,
    encode_frame:
        unsafe extern "C" fn(*mut EncoderObject, *const SourcePicture, *mut c_void) -> c_int,
    encode_parameter_sets: unsafe extern "C" fn(*mut EncoderObject, *mut c_void) -> c_int,
    force_intra_frame: unsafe extern "C" fn(*mut EncoderObject, bool, c_int) -> c_int,
    set_option: unsafe extern "C" fn(*mut EncoderObject, c_int, *mut c_void) -> c_int,
    get_option: unsafe extern "C" fn(*mut EncoderObject, c_int, *mut c_void) -> c_int,
}

/// A decoder object, as [`EncoderObject`] is for encoders.
type DecoderObject = *const DecoderVtable;

/// The `ISVCDecoder` virtual functions in declaration order, up to the ones used.
#[repr(C)]
struct DecoderVtable {
    initialize: unsafe extern "C" fn(*mut DecoderObject, *const DecodingParam) -> c_long,
    uninitialize: unsafe extern "C" fn(*mut DecoderObject) -> c_long,
    decode_frame: unsafe extern "C" fn() -> c_int,
    decode_frame_no_delay: unsafe extern "C" fn(
        *mut DecoderObject,
        *const u8,
        c_int,
        *mut [*mut u8; 3],
        *mut BufferInfo,
    ) -> c_int,
    decode_frame2: unsafe extern "C" fn() -> c_int,
    flush_frame: unsafe extern "C" fn() -> c_int,
    decode_parser: unsafe extern "C" fn() -> c_int,
    decode_frame_ex: unsafe extern "C" fn() -> c_int,
    set_option: unsafe extern "C" fn(*mut DecoderObject, c_int, *mut c_void) -> c_long,
    get_option: unsafe extern "C" fn(*mut DecoderObject, c_int, *mut c_void) -> c_long,
}

type CreateEncoder = unsafe extern "C" fn(*mut *mut EncoderObject) -> c_int;
type DestroyEncoder = unsafe extern "C" fn(*mut EncoderObject);
type CreateDecoder = unsafe extern "C" fn(*mut *mut DecoderObject) -> c_long;
type DestroyDecoder = unsafe extern "C" fn(*mut DecoderObject);
type GetVersion = unsafe extern "C" fn(*mut Version4);

/// A loaded OpenH264 of a supported version. Encoders and decoders keep it
/// loaded through their [`Arc`].
pub struct Library {
    create_encoder: CreateEncoder,
    destroy_encoder: DestroyEncoder,
    create_decoder: CreateDecoder,
    destroy_decoder: DestroyDecoder,
    version: Version,
    layout: Layout,
    path: String,
    // Last, so the functions above are never used after it unloads.
    _library: libloading::Library,
}

impl std::fmt::Debug for Library {
    /// The path and version.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("path", &self.path)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Library {
    /// Loads the system's OpenH264 by its [`LIBRARY_NAMES`], through the
    /// dynamic loader's search path (the user's own `LD_LIBRARY_PATH` and
    /// `ld.so` cache). A name that loads a refused version or something else
    /// does not stop the search; when none is usable, the first refusal is
    /// the error, or [`LoadError::Missing`] when nothing loaded.
    pub fn load() -> Result<Arc<Self>, LoadError> {
        Self::load_from(&LIBRARY_NAMES)
    }

    /// [`Library::load`] over `candidates`. Private: the safety of loading
    /// rests on the names being OpenH264's; the tests point it at a path
    /// that does not exist.
    fn load_from<S: AsRef<OsStr>>(candidates: &[S]) -> Result<Arc<Self>, LoadError> {
        let mut detail = String::from("nothing to try");
        let mut refused = None;
        for candidate in candidates {
            let name = candidate.as_ref();
            // SAFETY: loading runs the library's initializers. The names are
            // OpenH264's sonames, whose initializers only set up its own
            // statics; the loader resolves them as it would for any program
            // of this user.
            match unsafe { libloading::Library::new(name) } {
                Ok(library) => match Self::from_library(library, name.to_string_lossy().into()) {
                    Ok(loaded) => return Ok(loaded),
                    Err(error) => {
                        refused.get_or_insert(error);
                    }
                },
                Err(error) => detail = error.to_string(),
            }
        }
        if let Some(refused) = refused {
            return Err(refused);
        }
        Err(LoadError::Missing {
            tried: candidates
                .iter()
                .map(|c| c.as_ref().to_string_lossy().into_owned())
                .collect(),
            detail,
        })
    }

    /// Checks the version of a loaded `library` and takes its functions.
    fn from_library(library: libloading::Library, path: String) -> Result<Arc<Self>, LoadError> {
        let missing = |error: libloading::Error| LoadError::NotOpenH264 {
            path: path.clone(),
            detail: error.to_string(),
        };
        // SAFETY: each symbol is looked up by its exported C name and given
        // the signature `codec_api.h` declares for it since 1.x. The copied
        // function pointers stay valid while `library` is loaded, which the
        // struct guarantees by owning it.
        let (get_version, create_encoder, destroy_encoder, create_decoder, destroy_decoder) = unsafe {
            (
                *library
                    .get::<GetVersion>(b"WelsGetCodecVersionEx\0")
                    .map_err(missing)?,
                *library
                    .get::<CreateEncoder>(b"WelsCreateSVCEncoder\0")
                    .map_err(missing)?,
                *library
                    .get::<DestroyEncoder>(b"WelsDestroySVCEncoder\0")
                    .map_err(missing)?,
                *library
                    .get::<CreateDecoder>(b"WelsCreateDecoder\0")
                    .map_err(missing)?,
                *library
                    .get::<DestroyDecoder>(b"WelsDestroyDecoder\0")
                    .map_err(missing)?,
            )
        };
        let mut raw = Version4 {
            major: 0,
            minor: 0,
            revision: 0,
            reserved: 0,
        };
        // SAFETY: the function fills the four-integer struct it is given,
        // which has the same layout in every version.
        unsafe { get_version(&mut raw) };
        let version = Version {
            major: raw.major,
            minor: raw.minor,
            revision: raw.revision,
        };
        let Some(layout) = layout(version) else {
            return Err(LoadError::Unsupported {
                path,
                found: version,
            });
        };
        Ok(Arc::new(Self {
            create_encoder,
            destroy_encoder,
            create_decoder,
            destroy_decoder,
            version,
            layout,
            path,
            _library: library,
        }))
    }

    /// The loaded version.
    pub fn version(&self) -> Version {
        self.version
    }
}

/// How a new encoder starts; [`Encoder::set_bitrate`] and
/// [`Encoder::set_frame_rate`] change the rates later.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderConfig {
    /// Picture width; even.
    pub width: u32,
    /// Picture height; even.
    pub height: u32,
    /// Target bits per second.
    pub bitrate_bps: u32,
    /// The highest frame rate the rate control plans for.
    pub frame_rate: f32,
}

/// A picture in I420: a full-size luma plane and two quarter-size chroma planes.
#[derive(Debug, Clone, Copy)]
pub struct I420<'a> {
    /// Width in pixels; even.
    pub width: u32,
    /// Height in pixels; even.
    pub height: u32,
    /// The luma plane.
    pub y: &'a [u8],
    /// The blue-difference plane.
    pub u: &'a [u8],
    /// The red-difference plane.
    pub v: &'a [u8],
    /// Bytes between luma rows.
    pub y_stride: u32,
    /// Bytes between chroma rows.
    pub uv_stride: u32,
}

impl I420<'_> {
    /// Whether every plane holds its rows.
    fn fits(&self) -> bool {
        let plane = |len: usize, stride: u32, width: u32, rows: u32| {
            stride >= width
                && (stride as usize)
                    .checked_mul(rows as usize - 1)
                    .and_then(|before| before.checked_add(width as usize))
                    .is_some_and(|needed| len >= needed)
        };
        let (cw, ch) = (self.width / 2, self.height / 2);
        self.width >= 2
            && self.height >= 2
            && self.width.is_multiple_of(2)
            && self.height.is_multiple_of(2)
            && plane(self.y.len(), self.y_stride, self.width, self.height)
            && plane(self.u.len(), self.uv_stride, cw, ch)
            && plane(self.v.len(), self.uv_stride, cw, ch)
    }
}

/// What kind of frame an encode produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// Decodes on its own and resets the decoder.
    Idr,
    /// Intra-coded without a reset.
    Intra,
    /// Predicted from earlier frames.
    Predicted,
    /// The encoder produced nothing.
    Skipped,
}

/// One OpenH264 encoder.
pub struct Encoder {
    library: Arc<Library>,
    raw: NonNull<EncoderObject>,
    /// The output buffer, big enough for either layout.
    output: Box<FrameBsInfo<LayerBsInfoNew>>,
    config: EncoderConfig,
}

// SAFETY: an OpenH264 encoder has no thread affinity; it must only not be used
// from two threads at once, which `&mut self` on every call ensures.
unsafe impl Send for Encoder {}

impl std::fmt::Debug for Encoder {
    /// The configuration.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Zeroed memory for a C structure of plain integers, floats, booleans and
/// raw pointers, for which all-zero is a valid value.
fn zeroed<T>() -> Box<T> {
    // SAFETY: only used for the `repr(C)` structures above, whose fields are
    // integers, floats, `bool` and raw pointers; zero is valid for each.
    unsafe { Box::new(std::mem::zeroed()) }
}

impl Encoder {
    /// A screen-content encoder for `config`: one spatial layer, bitrate rate
    /// control without frame skipping (the host paces itself), an IDR only on
    /// the first frame and on request, constant parameter set ids, BT.709
    /// limited range signalled in the stream, and the library's logging off.
    pub fn new(library: &Arc<Library>, config: EncoderConfig) -> Result<Self, Error> {
        if config.width < 2
            || config.height < 2
            || !config.width.is_multiple_of(2)
            || !config.height.is_multiple_of(2)
        {
            return Err(Error::Picture("an encoder needs even sides of at least 2"));
        }
        let mut raw: *mut EncoderObject = std::ptr::null_mut();
        // SAFETY: the function writes a new encoder's address to `raw`.
        let code = unsafe { (library.create_encoder)(&mut raw) };
        check("WelsCreateSVCEncoder", code)?;
        // A success without an encoder is reported as code 0.
        let raw = NonNull::new(raw).ok_or(Error::Call {
            call: "WelsCreateSVCEncoder",
            code: 0,
        })?;
        let mut encoder = Self {
            library: library.clone(),
            raw,
            output: zeroed(),
            config,
        };
        let mut level = WELS_LOG_QUIET;
        encoder.set_option("trace level", ENCODER_OPTION_TRACE_LEVEL, &mut level)?;
        let mut params = zeroed::<EncParamExt>();
        // SAFETY: the encoder is alive; the library fills at most its own
        // version's prefix of the zeroed, larger structure.
        let code = unsafe {
            ((**encoder.raw.as_ptr()).get_default_params)(encoder.raw.as_ptr(), &mut *params)
        };
        check("GetDefaultParams", code)?;
        params.usage_type = SCREEN_CONTENT_REAL_TIME;
        params.pic_width = config.width as c_int;
        params.pic_height = config.height as c_int;
        params.target_bitrate = config.bitrate_bps.min(i32::MAX as u32) as c_int;
        params.rc_mode = RC_BITRATE_MODE;
        params.max_frame_rate = config.frame_rate;
        params.spatial_layer_num = 1;
        params.temporal_layer_num = 1;
        params.intra_period = 0;
        params.num_ref_frame = 1;
        params.sps_pps_id_strategy = CONSTANT_ID;
        params.entropy_coding_mode_flag = 0;
        params.enable_frame_skip = false;
        params.multiple_thread_idc = 1;
        params.enable_denoise = false;
        params.enable_background_detection = false;
        params.enable_adaptive_quant = false;
        params.enable_frame_cropping_flag = true;
        params.enable_scene_change_detect = false;
        let layer = &mut params.spatial_layers[0];
        layer.video_width = config.width as c_int;
        layer.video_height = config.height as c_int;
        layer.frame_rate = config.frame_rate;
        layer.spatial_bitrate = params.target_bitrate;
        layer.profile_idc = PRO_BASELINE;
        layer.video_signal_type_present = true;
        // Undefined video format, limited range, BT.709 primaries, transfer and matrix.
        layer.video_format = 5;
        layer.full_range = false;
        layer.color_description_present = true;
        layer.color_primaries = 1;
        layer.transfer_characteristics = 1;
        layer.color_matrix = 1;
        // SAFETY: the encoder is alive and `params` a full structure that
        // the library reads its version's prefix of.
        let code =
            unsafe { ((**encoder.raw.as_ptr()).initialize_ext)(encoder.raw.as_ptr(), &*params) };
        check("InitializeExt", code)?;
        Ok(encoder)
    }

    /// What the encoder was started with, rates as last set.
    pub fn config(&self) -> EncoderConfig {
        self.config
    }

    /// Sets an option whose value is `value`.
    fn set_option<T>(&mut self, call: &'static str, id: c_int, value: &mut T) -> Result<(), Error> {
        // SAFETY: the encoder is alive and `value` has the type the option
        // reads (the callers pair each id with its type).
        let code = unsafe {
            ((**self.raw.as_ptr()).set_option)(self.raw.as_ptr(), id, (value as *mut T).cast())
        };
        check(call, code)
    }

    /// Changes the target bitrate.
    pub fn set_bitrate(&mut self, bitrate_bps: u32) -> Result<(), Error> {
        let mut info = BitrateInfo {
            layer: SPATIAL_LAYER_ALL,
            bitrate: bitrate_bps.clamp(1, i32::MAX as u32) as c_int,
        };
        self.set_option("SetOption(bitrate)", ENCODER_OPTION_BITRATE, &mut info)?;
        self.config.bitrate_bps = bitrate_bps;
        Ok(())
    }

    /// Changes the frame rate the rate control plans for.
    pub fn set_frame_rate(&mut self, frame_rate: f32) -> Result<(), Error> {
        let mut rate = frame_rate;
        self.set_option(
            "SetOption(frame rate)",
            ENCODER_OPTION_FRAME_RATE,
            &mut rate,
        )?;
        self.config.frame_rate = frame_rate;
        Ok(())
    }

    /// Makes the next frame an IDR frame.
    pub fn force_idr(&mut self) -> Result<(), Error> {
        // SAFETY: the encoder is alive; -1 means every layer.
        let code =
            unsafe { ((**self.raw.as_ptr()).force_intra_frame)(self.raw.as_ptr(), true, -1) };
        check("ForceIntraFrame", code)
    }

    /// The average quantizer of the last frame encoded, from the encoder's
    /// statistics (the same structure in every 2.x).
    pub fn last_qp(&mut self) -> Result<u8, Error> {
        let mut statistics = zeroed::<EncoderStatistics>();
        // SAFETY: the encoder is alive; the option copies its statistics
        // into a structure of the layout every 2.x uses.
        let code = unsafe {
            ((**self.raw.as_ptr()).get_option)(
                self.raw.as_ptr(),
                ENCODER_OPTION_GET_STATISTICS,
                (&mut *statistics as *mut EncoderStatistics).cast(),
            )
        };
        check("GetOption(statistics)", code)?;
        Ok(statistics.average_frame_qp.min(51) as u8)
    }

    /// Encodes frames at the fixed quantizer `qp` (0 best to 51 worst) with
    /// rate control off, or with `None` under bitrate control again.
    pub fn set_fixed_qp(&mut self, qp: Option<u8>) -> Result<(), Error> {
        if let Some(qp) = qp {
            let mut params = zeroed::<EncParamExt>();
            // SAFETY: the encoder is alive; the library copies its version's
            // prefix of its parameters into the zeroed, larger structure.
            let code = unsafe {
                ((**self.raw.as_ptr()).get_option)(
                    self.raw.as_ptr(),
                    ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                    (&mut *params as *mut EncParamExt).cast(),
                )
            };
            check("GetOption(parameters)", code)?;
            params.spatial_layers[0].d_layer_qp = c_int::from(qp.min(51));
            params.rc_mode = RC_OFF_MODE;
            self.set_option(
                "SetOption(parameters)",
                ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                &mut *params,
            )?;
        }
        let mut mode = if qp.is_some() {
            RC_OFF_MODE
        } else {
            RC_BITRATE_MODE
        };
        self.set_option("SetOption(rate control)", ENCODER_OPTION_RC_MODE, &mut mode)
    }

    /// Encodes `picture`, captured at `timestamp_ms`, and appends its NAL
    /// units with start codes (Annex B) to `out`.
    pub fn encode(
        &mut self,
        picture: &I420<'_>,
        timestamp_ms: i64,
        out: &mut Vec<u8>,
    ) -> Result<FrameKind, Error> {
        if !picture.fits() {
            return Err(Error::Picture("the planes do not hold the picture"));
        }
        if (picture.width, picture.height) != (self.config.width, self.config.height) {
            return Err(Error::Picture("the picture is not the encoder's size"));
        }
        let source = SourcePicture {
            color_format: VIDEO_FORMAT_I420,
            stride: [
                picture.y_stride as c_int,
                picture.uv_stride as c_int,
                picture.uv_stride as c_int,
                0,
            ],
            // The library reads the source and never writes it.
            data: [
                picture.y.as_ptr().cast_mut(),
                picture.u.as_ptr().cast_mut(),
                picture.v.as_ptr().cast_mut(),
                std::ptr::null_mut(),
            ],
            pic_width: picture.width as c_int,
            pic_height: picture.height as c_int,
            time_stamp: timestamp_ms,
            psnr_y: false,
            psnr_u: false,
            psnr_v: false,
        };
        let output: *mut FrameBsInfo<LayerBsInfoNew> = &mut *self.output;
        // SAFETY: the encoder is alive, the planes hold the picture (checked
        // above), and the output buffer is large enough for the layout of
        // either version.
        let code = unsafe {
            ((**self.raw.as_ptr()).encode_frame)(self.raw.as_ptr(), &source, output.cast())
        };
        check("EncodeFrame", code)?;
        // SAFETY: the library wrote the output in its version's layout, which
        // `layout` names; the NAL lengths and buffer it points to stay valid
        // until the next call on this encoder.
        let frame_type = unsafe {
            match self.library.layout {
                Layout::From26 => collect(&*output, out),
                Layout::Before26 => collect(&*output.cast::<FrameBsInfo<LayerBsInfoOld>>(), out),
            }
        };
        Ok(match frame_type {
            VIDEO_FRAME_TYPE_IDR => FrameKind::Idr,
            VIDEO_FRAME_TYPE_I => FrameKind::Intra,
            VIDEO_FRAME_TYPE_SKIP => FrameKind::Skipped,
            _ => FrameKind::Predicted,
        })
    }
}

/// A layer of encoder output in either layout.
trait Layer {
    /// The NAL count, the lengths and the buffer.
    fn parts(&self) -> (c_int, *mut c_int, *mut u8);
}

impl Layer for LayerBsInfoOld {
    /// The NAL count, the lengths and the buffer.
    fn parts(&self) -> (c_int, *mut c_int, *mut u8) {
        (self.nal_count, self.nal_length_in_byte, self.bs_buf)
    }
}

impl Layer for LayerBsInfoNew {
    /// The NAL count, the lengths and the buffer.
    fn parts(&self) -> (c_int, *mut c_int, *mut u8) {
        (self.nal_count, self.nal_length_in_byte, self.bs_buf)
    }
}

/// Appends every NAL unit of `info` to `out` and returns its frame type.
///
/// # Safety
///
/// `info` was just written by `EncodeFrame` in the layout `L`, so its layer
/// count, NAL lengths and buffers describe memory the encoder owns.
unsafe fn collect<L: Layer>(info: &FrameBsInfo<L>, out: &mut Vec<u8>) -> c_int {
    let layers = (info.layer_num.max(0) as usize).min(MAX_LAYER_NUM_OF_FRAME);
    for layer in &info.layer_info[..layers] {
        let (count, lengths, buffer) = layer.parts();
        if count <= 0 || lengths.is_null() || buffer.is_null() {
            continue;
        }
        // SAFETY: the caller's contract: `lengths` holds `count` entries.
        let lengths = unsafe { std::slice::from_raw_parts(lengths, count as usize) };
        let total: usize = lengths.iter().map(|len| (*len).max(0) as usize).sum();
        // SAFETY: the layer's NAL units sit back to back in `buffer`.
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(buffer, total) });
    }
    info.frame_type
}

impl Drop for Encoder {
    /// Uninitializes and destroys the encoder.
    fn drop(&mut self) {
        // SAFETY: the encoder is alive and not used after this.
        unsafe {
            ((**self.raw.as_ptr()).uninitialize)(self.raw.as_ptr());
            (self.library.destroy_encoder)(self.raw.as_ptr());
        }
    }
}

/// A decoded picture in I420, borrowed from the decoder until its next call.
#[derive(Debug, Clone, Copy)]
pub struct Decoded<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The luma plane.
    pub y: &'a [u8],
    /// The blue-difference plane.
    pub u: &'a [u8],
    /// The red-difference plane.
    pub v: &'a [u8],
    /// Bytes between luma rows.
    pub y_stride: u32,
    /// Bytes between chroma rows.
    pub uv_stride: u32,
}

/// One OpenH264 decoder, with error concealment off so a broken stream fails.
pub struct Decoder {
    library: Arc<Library>,
    raw: NonNull<DecoderObject>,
}

// SAFETY: as for `Encoder`: no thread affinity, and `&mut self` on every call.
unsafe impl Send for Decoder {}

impl std::fmt::Debug for Decoder {
    /// Nothing worth showing.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

impl Decoder {
    /// A decoder for an AVC byte stream; refused with
    /// [`Error::DecoderTooOld`] for a library older than [`DECODER_OLDEST`].
    pub fn new(library: &Arc<Library>) -> Result<Self, Error> {
        if library.version < DECODER_OLDEST {
            return Err(Error::DecoderTooOld(library.version));
        }
        Self::any_version(library)
    }

    /// A decoder whatever the version: only the tests use older ones, to
    /// decode their own encoder's output and so read the pre-2.6 layout.
    fn any_version(library: &Arc<Library>) -> Result<Self, Error> {
        let mut raw: *mut DecoderObject = std::ptr::null_mut();
        // SAFETY: the function writes a new decoder's address to `raw`.
        let code = unsafe { (library.create_decoder)(&mut raw) };
        check("WelsCreateDecoder", code)?;
        // A success without a decoder is reported as code 0.
        let raw = NonNull::new(raw).ok_or(Error::Call {
            call: "WelsCreateDecoder",
            code: 0,
        })?;
        let decoder = Self {
            library: library.clone(),
            raw,
        };
        let mut level = WELS_LOG_QUIET;
        // SAFETY: the decoder is alive; the trace level is an int.
        let code = unsafe {
            ((**decoder.raw.as_ptr()).set_option)(
                decoder.raw.as_ptr(),
                DECODER_OPTION_TRACE_LEVEL,
                (&mut level as *mut c_int).cast(),
            )
        };
        check("SetOption(trace level)", code)?;
        let params = DecodingParam {
            file_name_restructed: std::ptr::null_mut(),
            cpu_load: 0,
            target_dq_layer: u8::MAX,
            ec_active_idc: ERROR_CON_DISABLE,
            parse_only: false,
            video_property: VideoProperty {
                size: std::mem::size_of::<VideoProperty>() as c_uint,
                bitstream_type: VIDEO_BITSTREAM_AVC,
            },
        };
        // SAFETY: the decoder is alive and `params` has the layout every 2.x reads.
        let code = unsafe { ((**decoder.raw.as_ptr()).initialize)(decoder.raw.as_ptr(), &params) };
        check("Initialize", code)?;
        Ok(decoder)
    }

    /// Decodes one access unit (Annex B) and returns its picture, or `None`
    /// when it held none (parameter sets only). An empty payload, and a
    /// picture with a side above [`MAX_SIDE`], are errors.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Decoded<'_>>, Error> {
        if data.is_empty() {
            return Err(Error::Picture("the payload is empty"));
        }
        let len =
            c_int::try_from(data.len()).map_err(|_| Error::Picture("the payload is too large"))?;
        let mut planes: [*mut u8; 3] = [std::ptr::null_mut(); 3];
        let mut info = BufferInfo {
            buffer_status: 0,
            in_bs_time_stamp: 0,
            out_yuv_time_stamp: 0,
            system_buffer: SysMemBuffer {
                width: 0,
                height: 0,
                format: 0,
                stride: [0; 2],
            },
            dst: [std::ptr::null_mut(); 3],
        };
        // SAFETY: the decoder is alive, `data` is readable for `len` bytes,
        // and the plane pointers and info are written by the call.
        let state = unsafe {
            ((**self.raw.as_ptr()).decode_frame_no_delay)(
                self.raw.as_ptr(),
                data.as_ptr(),
                len,
                &mut planes,
                &mut info,
            )
        };
        check("DecodeFrameNoDelay", state)?;
        if info.buffer_status != 1 {
            return Ok(None);
        }
        let buffer = info.system_buffer;
        let (width, height) = (buffer.width, buffer.height);
        let (y_stride, uv_stride) = (buffer.stride[0], buffer.stride[1]);
        if width <= 0 || height <= 0 || y_stride < width || uv_stride < (width + 1) / 2 {
            return Err(Error::Picture("the decoder returned a picture of no size"));
        }
        let (width_px, height_px) = (width as u32, height as u32);
        if width_px > MAX_SIDE || height_px > MAX_SIDE {
            return Err(Error::TooLarge {
                width: width_px,
                height: height_px,
            });
        }
        if planes.iter().any(|plane| plane.is_null()) {
            return Err(Error::Picture("the decoder returned no planes"));
        }
        let rows = |stride: c_int, count: c_int| stride as usize * count as usize;
        let chroma_rows = (height + 1) / 2;
        // SAFETY: a ready buffer has three planes of `height` (luma) and
        // `height / 2` rounded up (chroma) rows at their strides, owned by
        // the decoder until its next call, which `&mut self` holds off for
        // the lifetime of the result.
        let (y, u, v) = unsafe {
            (
                std::slice::from_raw_parts(planes[0], rows(y_stride, height)),
                std::slice::from_raw_parts(planes[1], rows(uv_stride, chroma_rows)),
                std::slice::from_raw_parts(planes[2], rows(uv_stride, chroma_rows)),
            )
        };
        Ok(Some(Decoded {
            width: width as u32,
            height: height as u32,
            y,
            u,
            v,
            y_stride: y_stride as u32,
            uv_stride: uv_stride as u32,
        }))
    }
}

impl Drop for Decoder {
    /// Uninitializes and destroys the decoder.
    fn drop(&mut self) {
        // SAFETY: the decoder is alive and not used after this.
        unsafe {
            ((**self.raw.as_ptr()).uninitialize)(self.raw.as_ptr());
            (self.library.destroy_decoder)(self.raw.as_ptr());
        }
    }
}

/// `Ok` for a zero return, the call's error otherwise.
fn check(call: &'static str, code: impl Into<i64>) -> Result<(), Error> {
    match code.into() {
        0 => Ok(()),
        code => Err(Error::Call { call, code }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version with `minor` and `revision` of major 2.
    fn two(minor: u32, revision: u32) -> Version {
        Version {
            major: 2,
            minor,
            revision,
        }
    }

    /// What distributions ship (2.2 in Ubuntu 22.04 to 2.7 in Arch) is
    /// accepted with the right output layout; anything else is refused,
    /// since its structures are not known.
    #[test]
    fn versions_outside_2_2_to_2_7_are_refused() {
        assert_eq!(layout(two(2, 0)), Some(Layout::Before26));
        assert_eq!(layout(two(3, 1)), Some(Layout::Before26));
        assert_eq!(layout(two(5, 0)), Some(Layout::Before26));
        assert_eq!(layout(two(6, 0)), Some(Layout::From26));
        assert_eq!(layout(two(7, 3)), Some(Layout::From26));
        for refused in [
            two(1, 1),
            two(8, 0),
            Version {
                major: 1,
                minor: 8,
                revision: 0,
            },
            Version {
                major: 3,
                minor: 0,
                revision: 0,
            },
        ] {
            assert!(!supported(refused), "{refused}");
        }
    }

    /// The output layouts differ exactly by the PSNR values 2.6 added, which
    /// is what moves every later layer and the frame type.
    #[test]
    fn the_two_output_layouts_have_the_sizes_of_the_c_headers() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(std::mem::size_of::<LayerBsInfoOld>(), 40);
            assert_eq!(std::mem::size_of::<LayerBsInfoNew>(), 56);
        }
        assert!(
            std::mem::size_of::<FrameBsInfo<LayerBsInfoNew>>()
                > std::mem::size_of::<FrameBsInfo<LayerBsInfoOld>>()
        );
    }

    /// A loader pointed at a path that does not exist reports the library
    /// missing, with what it tried.
    #[test]
    fn a_missing_library_is_reported_with_what_was_tried() {
        let missing = "/nonexistent/devlanes-test/libopenh264.so.8";
        match Library::load_from(&[missing]) {
            Err(LoadError::Missing { tried, detail }) => {
                assert_eq!(tried, vec![missing.to_owned()]);
                assert!(!detail.is_empty());
            }
            other => panic!("expected a missing library, got {other:?}"),
        }
    }

    /// The installed library encodes a picture and decodes its own output,
    /// whichever output layout its version writes (CI's Ubuntu package is
    /// 2.4, Cisco's binary 2.6), and the public decoder refuses versions
    /// before 2.6. Skips, printing why, without OpenH264, unless
    /// `DEVLANES_REQUIRE_H264` names `encode`.
    #[test]
    fn the_installed_library_decodes_its_own_output() {
        let library = match Library::load() {
            Ok(library) => library,
            Err(error) => {
                let required = std::env::var("DEVLANES_REQUIRE_H264").unwrap_or_default();
                assert!(
                    !required.contains("encode"),
                    "OpenH264 is required: {error}"
                );
                eprintln!("skipped: {error}");
                return;
            }
        };
        assert_eq!(
            matches!(Decoder::new(&library), Err(Error::DecoderTooOld(_))),
            library.version() < DECODER_OLDEST,
            "{}",
            library.version()
        );
        let (width, height) = (64u32, 48u32);
        let y: Vec<u8> = (0..width * height)
            .map(|i| 16 + ((i % width) * 3) as u8)
            .collect();
        let chroma = vec![128u8; (width * height / 4) as usize];
        let picture = I420 {
            width,
            height,
            y: &y,
            u: &chroma,
            v: &chroma,
            y_stride: width,
            uv_stride: width / 2,
        };
        let config = EncoderConfig {
            width,
            height,
            bitrate_bps: 2_000_000,
            frame_rate: 30.0,
        };
        let mut encoder = Encoder::new(&library, config).expect("an encoder");
        let mut decoder = Decoder::any_version(&library).expect("a decoder");
        for frame in 0..3 {
            let mut out = Vec::new();
            let kind = encoder
                .encode(&picture, frame * 33, &mut out)
                .expect("encodes");
            assert_eq!(kind == FrameKind::Idr, frame == 0, "frame {frame}");
            let decoded = decoder.decode(&out).expect("decodes").expect("a picture");
            assert_eq!((decoded.width, decoded.height), (width, height));
            let row = &decoded.y[..width as usize];
            for (got, want) in row.iter().zip(&y[..width as usize]) {
                assert!(got.abs_diff(*want) <= 8, "{got} for {want}");
            }
        }
    }
}
