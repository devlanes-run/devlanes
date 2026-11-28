//! H.264 through Media Foundation: the encoder transforms Windows has (a GPU
//! vendor's hardware encoder where there is one, Microsoft's software encoder
//! otherwise) and Microsoft's decoder, in low-latency mode, with frames in
//! system memory as NV12.
//!
//! Threads: Media Foundation's objects live in the multithreaded apartment.
//! The first use in a process calls `CoIncrementMTAUsage`, which keeps that
//! apartment alive for the process without joining the calling thread to it,
//! so every thread that never initialized COM (the host's and the viewer's
//! codec threads) uses the objects as an implicit member, and a thread that
//! initialized COM single-threaded (the viewer's winit thread) is never
//! changed. Nothing here runs on such a thread.
//!
//! The transforms' free functions (`MFStartup`, `MFTEnumEx`, `MFCreateSample`
//! and friends) are looked up in `mfplat.dll` at run time instead of being
//! imported, so a Windows without Media Foundation (the N editions) still
//! starts the binary and reports H.264 unavailable.
//!
//! Rate: encoders run in the quality rate-control mode with a quantizer per
//! frame, which the caller derives from its pacer, since a hardware encoder
//! under constant-bitrate control ignored both a raised bitrate and a
//! quantizer for a single frame (NVIDIA's codes an unchanged picture as
//! skipped blocks whatever it is asked). A transform that refuses that mode
//! runs at a constant bitrate instead.
//!
//! Calls are synchronous by contract: an asynchronous (hardware) transform is
//! driven through its events until the output for the frame just given is
//! there, with [`ASYNC_TIMEOUT`] as the limit.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HMODULE, VARIANT_TRUE};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264DecoderMFT, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncVideoEncodeQP,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFMediaBuffer, IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFShutdown, IMFTransform,
    METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE, MF_E_NOTACCEPTING,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT,
    MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFMediaType_Video,
    MFSTARTUP_LITE, MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_Base,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoIncrementMTAUsage, CoTaskMemFree,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0_0, VT_BOOL, VT_UI4, VT_UI8};
use windows::core::{GUID, HRESULT, Interface, PWSTR, s, w};

/// How long an asynchronous transform may take to want a frame or to give
/// back its output before the call fails.
pub const ASYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a Media Foundation call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Media Foundation, or the transform asked for, is not on this Windows.
    #[error("Media Foundation is not available: {0}")]
    Unavailable(String),
    /// A call returned an error.
    #[error("Media Foundation {call} failed")]
    Call {
        /// The call.
        call: &'static str,
        /// What it returned.
        #[source]
        source: windows::core::Error,
    },
    /// An asynchronous transform gave nothing back within [`ASYNC_TIMEOUT`].
    #[error("the H.264 transform did not answer within {}s", ASYNC_TIMEOUT.as_secs())]
    Timeout,
    /// Input that does not hold the picture it is said to be.
    #[error("invalid picture: {0}")]
    Picture(&'static str),
}

/// Names the call a COM error came from.
fn call(call: &'static str) -> impl FnOnce(windows::core::Error) -> Error {
    move |source| Error::Call { call, source }
}

/// What `GetProcAddress` returns.
type Proc = unsafe extern "system" fn() -> isize;
type StartupFn = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateFn = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateBufferFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;
type EnumFn = unsafe extern "system" fn(
    GUID,
    u32,
    *const MFT_REGISTER_TYPE_INFO,
    *const MFT_REGISTER_TYPE_INFO,
    *mut *mut *mut c_void,
    *mut u32,
) -> HRESULT;

/// The `mfplat.dll` functions, looked up once per process.
struct Api {
    create_sample: CreateFn,
    create_memory_buffer: CreateBufferFn,
    create_media_type: CreateFn,
    enum_ex: EnumFn,
}

/// Media Foundation, started once for the process; an error says why not.
fn api() -> Result<&'static Api, Error> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(start)
        .as_ref()
        .map_err(|why| Error::Unavailable(why.clone()))
}

/// Loads `mfplat.dll` from System32, looks its functions up, keeps the
/// multithreaded apartment alive for the process and starts Media Foundation.
fn start() -> Result<Api, String> {
    // SAFETY: loads a system DLL by name from System32 only; the module is
    // never freed, so the looked-up functions stay valid for the process.
    let module: HMODULE =
        unsafe { LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map_err(|error| format!("mfplat.dll does not load: {error}"))?;
    let find = |name: windows::core::PCSTR, label: &str| {
        // SAFETY: `module` is loaded and `name` a NUL-terminated export name.
        unsafe { GetProcAddress(module, name) }.ok_or_else(|| format!("mfplat.dll has no {label}"))
    };
    let startup = find(s!("MFStartup"), "MFStartup")?;
    let create_sample = find(s!("MFCreateSample"), "MFCreateSample")?;
    let create_memory_buffer = find(s!("MFCreateMemoryBuffer"), "MFCreateMemoryBuffer")?;
    let create_media_type = find(s!("MFCreateMediaType"), "MFCreateMediaType")?;
    let enum_ex = find(s!("MFTEnumEx"), "MFTEnumEx")?;
    // SAFETY: each export has the signature `mfapi.h` declares for it, which
    // the function pointer types above copy.
    let api = unsafe {
        Api {
            create_sample: std::mem::transmute::<Proc, CreateFn>(create_sample),
            create_memory_buffer: std::mem::transmute::<Proc, CreateBufferFn>(create_memory_buffer),
            create_media_type: std::mem::transmute::<Proc, CreateFn>(create_media_type),
            enum_ex: std::mem::transmute::<Proc, EnumFn>(enum_ex),
        }
    };
    // SAFETY: the cookie is never released, so the apartment lives as long
    // as the process; the call itself touches no thread's apartment.
    unsafe { CoIncrementMTAUsage() }.map_err(|error| format!("COM did not start: {error}"))?;
    // SAFETY: as above for the signature; the light startup needs no sockets.
    let started =
        unsafe { std::mem::transmute::<Proc, StartupFn>(startup)(MF_VERSION, MFSTARTUP_LITE) };
    started
        .ok()
        .map_err(|error| format!("MFStartup failed: {error}"))?;
    Ok(api)
}

/// A new media type.
fn media_type(api: &Api) -> Result<IMFMediaType, Error> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: the function writes a new reference to `raw` on success, which
    // `from_raw` takes over.
    unsafe {
        (api.create_media_type)(&mut raw)
            .ok()
            .map_err(call("MFCreateMediaType"))?;
        Ok(IMFMediaType::from_raw(raw))
    }
}

/// A new sample holding a copy of `bytes`, or an empty buffer of `capacity`
/// bytes when `bytes` is `None`.
fn sample(api: &Api, bytes: Option<&[u8]>, capacity: usize) -> Result<IMFSample, Error> {
    let size = bytes.map_or(capacity, <[u8]>::len);
    let size = u32::try_from(size).map_err(|_| Error::Picture("more than 4 GiB"))?;
    let (mut raw_sample, mut raw_buffer) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: the creators write new references that `from_raw` takes over;
    // the buffer is locked for exactly `size` bytes while they are written.
    unsafe {
        (api.create_sample)(&mut raw_sample)
            .ok()
            .map_err(call("MFCreateSample"))?;
        let sample = IMFSample::from_raw(raw_sample);
        (api.create_memory_buffer)(size, &mut raw_buffer)
            .ok()
            .map_err(call("MFCreateMemoryBuffer"))?;
        let buffer = IMFMediaBuffer::from_raw(raw_buffer);
        if let Some(bytes) = bytes {
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(call("IMFMediaBuffer::Lock"))?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock().map_err(call("IMFMediaBuffer::Unlock"))?;
            buffer
                .SetCurrentLength(size)
                .map_err(call("IMFMediaBuffer::SetCurrentLength"))?;
        }
        sample
            .AddBuffer(&buffer)
            .map_err(call("IMFSample::AddBuffer"))?;
        Ok(sample)
    }
}

/// The bytes of `sample`, appended to `out`.
fn read_sample(sample: &IMFSample, out: &mut Vec<u8>) -> Result<(), Error> {
    // SAFETY: the contiguous buffer is locked while its current length is copied.
    unsafe {
        let buffer = sample
            .ConvertToContiguousBuffer()
            .map_err(call("IMFSample::ConvertToContiguousBuffer"))?;
        let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
        buffer
            .Lock(&mut data, None, Some(&mut len))
            .map_err(call("IMFMediaBuffer::Lock"))?;
        out.extend_from_slice(std::slice::from_raw_parts(data, len as usize));
        buffer.Unlock().map_err(call("IMFMediaBuffer::Unlock"))?;
    }
    Ok(())
}

/// A `VT_UI4` variant.
fn variant_u32(value: u32) -> VARIANT {
    let mut variant = VARIANT::default();
    let mut inner = VARIANT_0_0 {
        vt: VT_UI4,
        ..Default::default()
    };
    inner.Anonymous.ulVal = value;
    variant.Anonymous.Anonymous = ManuallyDrop::new(inner);
    variant
}

/// A `VT_UI8` variant.
fn variant_u64(value: u64) -> VARIANT {
    let mut variant = VARIANT::default();
    let mut inner = VARIANT_0_0 {
        vt: VT_UI8,
        ..Default::default()
    };
    inner.Anonymous.ullVal = value;
    variant.Anonymous.Anonymous = ManuallyDrop::new(inner);
    variant
}

/// A `VT_BOOL` variant that is true.
fn variant_true() -> VARIANT {
    let mut variant = VARIANT::default();
    let mut inner = VARIANT_0_0 {
        vt: VT_BOOL,
        ..Default::default()
    };
    inner.Anonymous.boolVal = VARIANT_TRUE;
    variant.Anonymous.Anonymous = ManuallyDrop::new(inner);
    variant
}

/// The transforms registered for H.264 encoding from NV12 with `flags`,
/// best first, with their names.
fn encoders(api: &Api, flags: u32) -> Result<Vec<(String, IMFActivate)>, Error> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let (mut array, mut count) = (std::ptr::null_mut(), 0u32);
    // SAFETY: on success the function returns `count` references in a
    // CoTaskMem array; each is taken over by `from_raw` and the array freed.
    unsafe {
        (api.enum_ex)(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags | MFT_ENUM_FLAG_SORTANDFILTER.0 as u32,
            &input,
            &output,
            &mut array,
            &mut count,
        )
        .ok()
        .map_err(call("MFTEnumEx"))?;
        if array.is_null() {
            return Ok(Vec::new());
        }
        let found = std::slice::from_raw_parts(array, count as usize)
            .iter()
            .filter(|raw| !raw.is_null())
            .map(|raw| IMFActivate::from_raw(*raw))
            .collect::<Vec<_>>();
        CoTaskMemFree(Some(array as *const c_void));
        Ok(found
            .into_iter()
            .map(|activate| (friendly_name(&activate), activate))
            .collect())
    }
}

/// A transform's registered name, or `?`.
fn friendly_name(activate: &IMFActivate) -> String {
    let (mut text, mut len) = (PWSTR::null(), 0u32);
    // SAFETY: on success the string is a CoTaskMem allocation of `len`
    // characters, read once and freed.
    unsafe {
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut len)
            .is_err()
        {
            return "?".to_owned();
        }
        let name = text.to_string().unwrap_or_else(|_| "?".to_owned());
        CoTaskMemFree(Some(text.0 as *const c_void));
        name
    }
}

/// Whether this machine has a hardware H.264 encoder transform. Blocking.
pub fn has_hardware_encoder() -> Result<bool, Error> {
    Ok(!encoders(api()?, MFT_ENUM_FLAG_HARDWARE.0 as u32)?.is_empty())
}

/// Whether this machine has a software H.264 encoder transform. Blocking.
pub fn has_software_encoder() -> Result<bool, Error> {
    Ok(!encoders(api()?, MFT_ENUM_FLAG_SYNCMFT.0 as u32)?.is_empty())
}

/// How a new encoder starts; [`Encoder::set_bitrate`] changes the rate later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Picture width; even.
    pub width: u32,
    /// Picture height; even.
    pub height: u32,
    /// Target bits per second.
    pub bitrate_bps: u32,
    /// Frames per second the stream is declared at.
    pub frame_rate: u32,
}

/// An H.264 encoder transform, set up for one picture size.
pub struct Encoder {
    api: &'static Api,
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    /// Present for an asynchronous transform, driven through its events.
    events: Option<IMFMediaEventGenerator>,
    /// Input the asynchronous transform asked for and has not had.
    wanted: u32,
    provides_samples: bool,
    output_size: usize,
    /// Under the quality rate-control mode, with a quantizer per frame.
    constant_qp: bool,
    config: EncoderConfig,
    hardware: bool,
    name: String,
}

// SAFETY: the transform and its interfaces belong to the multithreaded
// apartment, so any thread may call them; `&mut self` keeps calls to one at
// a time, which is all a transform supports.
unsafe impl Send for Encoder {}

impl std::fmt::Debug for Encoder {
    /// The transform's name and configuration.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("name", &self.name)
            .field("hardware", &self.hardware)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// The best hardware (`hardware`) or software H.264 encoder that accepts
    /// `config`, in low-latency mode with a quantizer per frame (or constant
    /// bitrate), the baseline profile and no B-frames. Transforms that refuse the setup (one that
    /// needs a Direct3D device, say) are passed over for the next.
    pub fn new(config: EncoderConfig, hardware: bool) -> Result<Self, Error> {
        if config.width < 2
            || config.height < 2
            || !config.width.is_multiple_of(2)
            || !config.height.is_multiple_of(2)
        {
            return Err(Error::Picture("an encoder needs even sides of at least 2"));
        }
        let api = api()?;
        // Hardware transforms activated from two threads at once failed with
        // E_UNEXPECTED on an NVIDIA driver; one at a time they start.
        static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one_at_a_time = STARTING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let flags = match hardware {
            true => MFT_ENUM_FLAG_HARDWARE.0 as u32,
            false => MFT_ENUM_FLAG_SYNCMFT.0 as u32,
        };
        let mut last = Error::Unavailable(match hardware {
            true => "no hardware H.264 encoder".to_owned(),
            false => "no software H.264 encoder".to_owned(),
        });
        for (name, activate) in encoders(api, flags)? {
            match Self::start(api, &activate, config, hardware, name) {
                Ok(encoder) => return Ok(encoder),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// Activates and configures one transform.
    fn start(
        api: &'static Api,
        activate: &IMFActivate,
        config: EncoderConfig,
        hardware: bool,
        name: String,
    ) -> Result<Self, Error> {
        let rate = (u64::from(config.frame_rate.max(1)) << 32) | 1;
        let size = (u64::from(config.width) << 32) | u64::from(config.height);
        // SAFETY: every call is on interfaces this function holds, with
        // attribute keys of the documented types.
        unsafe {
            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(call("IMFActivate::ActivateObject"))?;
            let attributes = transform.GetAttributes().ok();
            let asynchronous = attributes
                .as_ref()
                .and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok())
                == Some(1);
            if let Some(attributes) = &attributes {
                if asynchronous {
                    attributes
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(call("MF_TRANSFORM_ASYNC_UNLOCK"))?;
                }
                // Not every transform reads it; the codec API below is the main switch.
                let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let codec = transform.cast::<ICodecAPI>().ok();
            let mut constant_qp = false;
            if let Some(codec) = &codec {
                // Each is a preference a transform may not support.
                let _ = codec.SetValue(&CODECAPI_AVLowLatencyMode, &variant_true());
                constant_qp = codec
                    .SetValue(
                        &CODECAPI_AVEncCommonRateControlMode,
                        &variant_u32(eAVEncCommonRateControlMode_Quality.0 as u32),
                    )
                    .is_ok();
                if !constant_qp {
                    let _ = codec.SetValue(
                        &CODECAPI_AVEncCommonRateControlMode,
                        &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                    );
                    let _ = codec.SetValue(
                        &CODECAPI_AVEncCommonMeanBitRate,
                        &variant_u32(config.bitrate_bps),
                    );
                }
                let _ = codec.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
            }
            let output = media_type(api)?;
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(call("output type"))?;
            output
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(call("output type"))?;
            output
                .SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate_bps)
                .map_err(call("output type"))?;
            output
                .SetUINT64(&MF_MT_FRAME_SIZE, size)
                .map_err(call("output type"))?;
            output
                .SetUINT64(&MF_MT_FRAME_RATE, rate)
                .map_err(call("output type"))?;
            output
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(call("output type"))?;
            output
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                .map_err(call("output type"))?;
            output
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)
                .map_err(call("output type"))?;
            transform
                .SetOutputType(0, &output, 0)
                .map_err(call("IMFTransform::SetOutputType"))?;
            let input = media_type(api)?;
            input
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(call("input type"))?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(call("input type"))?;
            input
                .SetUINT64(&MF_MT_FRAME_SIZE, size)
                .map_err(call("input type"))?;
            input
                .SetUINT64(&MF_MT_FRAME_RATE, rate)
                .map_err(call("input type"))?;
            input
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(call("input type"))?;
            input
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)
                .map_err(call("input type"))?;
            transform
                .SetInputType(0, &input, 0)
                .map_err(call("IMFTransform::SetInputType"))?;
            let events = match asynchronous {
                true => Some(
                    transform
                        .cast::<IMFMediaEventGenerator>()
                        .map_err(call("IMFMediaEventGenerator"))?,
                ),
                false => None,
            };
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(call("begin streaming"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(call("start of stream"))?;
            let info = transform
                .GetOutputStreamInfo(0)
                .map_err(call("IMFTransform::GetOutputStreamInfo"))?;
            let provides = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
            Ok(Self {
                api,
                transform,
                codec,
                events,
                wanted: 0,
                provides_samples: info.dwFlags & provides != 0,
                output_size: (info.cbSize as usize)
                    .max(config.width as usize * config.height as usize * 2),
                constant_qp,
                config,
                hardware,
                name,
            })
        }
    }

    /// The transform's registered name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The configuration, the bitrate as last set.
    pub fn config(&self) -> EncoderConfig {
        self.config
    }

    /// Changes the target bitrate.
    pub fn set_bitrate(&mut self, bitrate_bps: u32) -> Result<(), Error> {
        if let Some(codec) = &self.codec {
            // SAFETY: a documented codec property with a `VT_UI4` value.
            unsafe { codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bitrate_bps)) }
                .map_err(call("CODECAPI_AVEncCommonMeanBitRate"))?;
        }
        self.config.bitrate_bps = bitrate_bps;
        Ok(())
    }

    /// Whether the transform takes a quantizer per frame; one that does not
    /// is under constant-bitrate control and ignores the `qp` of
    /// [`Encoder::encode`].
    pub fn constant_qp(&self) -> bool {
        self.constant_qp
    }

    /// Encodes one NV12 picture (luma rows `width` bytes apart, then the
    /// interleaved chroma) captured at `time`, appending its H.264 byte
    /// stream to `out`. `keyframe` forces an IDR frame; `qp` (0 best to 51
    /// worst) is the frame's quantizer. Returns whether the output is a
    /// keyframe, or `None` when the transform gave nothing for this frame.
    pub fn encode(
        &mut self,
        nv12: &[u8],
        time: Duration,
        duration: Duration,
        keyframe: bool,
        qp: u8,
        out: &mut Vec<u8>,
    ) -> Result<Option<bool>, Error> {
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        if nv12.len() != w * h * 3 / 2 {
            return Err(Error::Picture(
                "the buffer is not an NV12 picture of the encoder's size",
            ));
        }
        if keyframe {
            let Some(codec) = &self.codec else {
                return Err(Error::Unavailable(
                    "the encoder cannot force a keyframe".to_owned(),
                ));
            };
            // SAFETY: a documented codec property with a `VT_UI4` value.
            unsafe { codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1)) }
                .map_err(call("CODECAPI_AVEncVideoForceKeyFrame"))?;
        }
        let input = sample(self.api, Some(nv12), 0)?;
        let ticks = |d: Duration| i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX);
        // SAFETY: attributes of a sample this function owns.
        unsafe {
            input
                .SetSampleTime(ticks(time))
                .map_err(call("IMFSample::SetSampleTime"))?;
            input
                .SetSampleDuration(ticks(duration))
                .map_err(call("IMFSample::SetSampleDuration"))?;
        }
        if let (true, Some(codec)) = (self.constant_qp, &self.codec) {
            // The quantizer of every frame type is the low 16 bits.
            // SAFETY: a documented codec property with a `VT_UI8` value,
            // valid in the quality rate-control mode the encoder runs in.
            unsafe {
                codec.SetValue(
                    &CODECAPI_AVEncVideoEncodeQP,
                    &variant_u64(u64::from(qp.min(51))),
                )
            }
            .map_err(call("CODECAPI_AVEncVideoEncodeQP"))?;
        }
        out.clear();
        match self.events.is_some() {
            true => self.encode_async(&input, out),
            false => self.encode_sync(&input, out),
        }
    }

    /// Feeds a synchronous transform and collects what it gives back.
    fn encode_sync(&mut self, input: &IMFSample, out: &mut Vec<u8>) -> Result<Option<bool>, Error> {
        let mut key = None;
        // SAFETY: the transform is set up and `input` a filled sample.
        let given = unsafe { self.transform.ProcessInput(0, input, 0) };
        match given {
            Ok(()) => {}
            // Output still inside it: take that first, then give again.
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                while let Some(k) = self.take_output(out)? {
                    key = Some(k);
                }
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, input, 0) }
                    .map_err(call("IMFTransform::ProcessInput"))?;
            }
            Err(error) => return Err(call("IMFTransform::ProcessInput")(error)),
        }
        while let Some(k) = self.take_output(out)? {
            key = Some(key.unwrap_or(false) || k);
        }
        Ok(key)
    }

    /// Drives an asynchronous transform through its events: gives the frame
    /// once it asks for input, and returns once the output for it is there.
    fn encode_async(
        &mut self,
        input: &IMFSample,
        out: &mut Vec<u8>,
    ) -> Result<Option<bool>, Error> {
        let deadline = Instant::now() + ASYNC_TIMEOUT;
        while self.wanted == 0 {
            match self.next_event(deadline)? {
                Event::NeedInput => self.wanted += 1,
                // Output of no frame of ours; nothing to keep.
                Event::HaveOutput => {
                    let mut stray = Vec::new();
                    self.take_output(&mut stray)?;
                }
                Event::Other => {}
            }
        }
        self.wanted -= 1;
        // SAFETY: the transform asked for input and `input` is a filled sample.
        unsafe { self.transform.ProcessInput(0, input, 0) }
            .map_err(call("IMFTransform::ProcessInput"))?;
        loop {
            match self.next_event(deadline)? {
                Event::NeedInput => self.wanted += 1,
                Event::HaveOutput => {
                    if let Some(key) = self.take_output(out)? {
                        return Ok(Some(key));
                    }
                }
                Event::Other => {}
            }
        }
    }

    /// The next event of an asynchronous transform, polled until `deadline`.
    fn next_event(&self, deadline: Instant) -> Result<Event, Error> {
        let Some(events) = &self.events else {
            return Ok(Event::Other);
        };
        loop {
            // SAFETY: polling the transform's own event queue without waiting.
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    // SAFETY: reading the type of an event this call owns.
                    let kind =
                        unsafe { event.GetType() }.map_err(call("IMFMediaEvent::GetType"))?;
                    return Ok(match kind {
                        k if k == METransformNeedInput.0 as u32 => Event::NeedInput,
                        k if k == METransformHaveOutput.0 as u32 => Event::HaveOutput,
                        _ => Event::Other,
                    });
                }
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                Err(error) => return Err(call("IMFMediaEventGenerator::GetEvent")(error)),
            }
        }
    }

    /// Takes one output sample, appending its bytes to `out`; `None` when
    /// the transform needs more input first.
    fn take_output(&mut self, out: &mut Vec<u8>) -> Result<Option<bool>, Error> {
        let output = match self.provides_samples {
            true => None,
            false => Some(sample(self.api, None, self.output_size)?),
        };
        take(&self.transform, output, out).map(|got| got.map(|(key, _)| key))
    }
}

/// What an asynchronous transform said.
enum Event {
    NeedInput,
    HaveOutput,
    Other,
}

/// Calls `ProcessOutput` with `output` (or a sample of the transform's own)
/// and appends what came out to `out`: whether it is a keyframe, and the
/// sample. `None` when the transform needs more input.
fn take(
    transform: &IMFTransform,
    output: Option<IMFSample>,
    out: &mut Vec<u8>,
) -> Result<Option<(bool, IMFSample)>, Error> {
    let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(output),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None),
    }];
    let mut status = 0;
    // SAFETY: one output buffer for stream 0; the transform fills or replaces
    // its sample, and the references left in it are taken back below.
    let result = unsafe { transform.ProcessOutput(0, &mut buffers, &mut status) };
    let sample = ManuallyDrop::into_inner(std::mem::replace(
        &mut buffers[0].pSample,
        ManuallyDrop::new(None),
    ));
    drop(ManuallyDrop::into_inner(std::mem::replace(
        &mut buffers[0].pEvents,
        ManuallyDrop::new(None),
    )));
    match result {
        Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
        Err(error) => Err(Error::Call {
            call: "IMFTransform::ProcessOutput",
            source: error,
        }),
        Ok(()) => {
            let Some(sample) = sample else {
                return Ok(None);
            };
            // SAFETY: an attribute of the sample just returned.
            let key = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) == 1;
            read_sample(&sample, out)?;
            Ok(Some((key, sample)))
        }
    }
}

impl Drop for Encoder {
    /// Shuts an asynchronous transform down, which releases its device.
    fn drop(&mut self) {
        if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
            // SAFETY: the transform is not used after this.
            let _ = unsafe { shutdown.Shutdown() };
        }
    }
}

/// A decoded NV12 picture: `stride * height` bytes of luma, then the
/// interleaved chroma rows, `stride` bytes apart. `width` and `height` are
/// the decoded size, which can be larger than the picture's (Microsoft's
/// decoder rounds the height up to a multiple of 16).
#[derive(Debug, Clone)]
pub struct Nv12Picture {
    /// The bytes.
    pub data: Vec<u8>,
    /// Decoded width.
    pub width: u32,
    /// Decoded height.
    pub height: u32,
    /// Bytes between rows.
    pub stride: u32,
}

/// Microsoft's H.264 decoder transform, in low-latency mode so each access
/// unit comes out at once.
pub struct Decoder {
    api: &'static Api,
    transform: IMFTransform,
    provides_samples: bool,
    output_size: usize,
    width: u32,
    height: u32,
    stride: u32,
}

// SAFETY: as for `Encoder`.
unsafe impl Send for Decoder {}

impl std::fmt::Debug for Decoder {
    /// The current output size.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Decoder {
    /// A decoder of an H.264 byte stream into NV12.
    pub fn new() -> Result<Self, Error> {
        let api = api()?;
        // SAFETY: every call is on interfaces this function holds, with
        // attribute keys of the documented types.
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER).map_err(
                    |error| {
                        Error::Unavailable(format!("the H.264 decoder does not start: {error}"))
                    },
                )?;
            if let Ok(attributes) = transform.GetAttributes() {
                let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let input = media_type(api)?;
            input
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(call("input type"))?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(call("input type"))?;
            transform
                .SetInputType(0, &input, 0)
                .map_err(call("IMFTransform::SetInputType"))?;
            let mut decoder = Self {
                api,
                transform,
                provides_samples: false,
                output_size: 0,
                width: 0,
                height: 0,
                stride: 0,
            };
            decoder.choose_output()?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(call("begin streaming"))?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(call("start of stream"))?;
            Ok(decoder)
        }
    }

    /// Picks NV12 among the output types the decoder offers now, and reads
    /// the picture size and stride from it. Called at the start and on every
    /// stream change (a new size).
    fn choose_output(&mut self) -> Result<(), Error> {
        for index in 0.. {
            // SAFETY: enumerating and setting the transform's own output types.
            unsafe {
                let offered = self
                    .transform
                    .GetOutputAvailableType(0, index)
                    .map_err(call("IMFTransform::GetOutputAvailableType"))?;
                if offered.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                self.transform
                    .SetOutputType(0, &offered, 0)
                    .map_err(call("IMFTransform::SetOutputType"))?;
                let size = offered.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                self.width = (size >> 32) as u32;
                self.height = size as u32;
                let stride = offered.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(0) as i32;
                self.stride = match stride {
                    s if s > 0 => s as u32,
                    _ => self.width,
                };
                let info = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map_err(call("IMFTransform::GetOutputStreamInfo"))?;
                let provides = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32;
                self.provides_samples = info.dwFlags & provides != 0;
                self.output_size =
                    (info.cbSize as usize).max(self.stride as usize * self.height as usize * 3 / 2);
                return Ok(());
            }
        }
        Err(Error::Unavailable(
            "the H.264 decoder offers no NV12 output".to_owned(),
        ))
    }

    /// Decodes one access unit of an H.264 byte stream and returns its
    /// picture, or `None` when it held none.
    pub fn decode(&mut self, data: &[u8], time: Duration) -> Result<Option<Nv12Picture>, Error> {
        let input = sample(self.api, Some(data), 0)?;
        // SAFETY: an attribute of a sample this function owns.
        unsafe { input.SetSampleTime(i64::try_from(time.as_nanos() / 100).unwrap_or(i64::MAX)) }
            .map_err(call("IMFSample::SetSampleTime"))?;
        let mut picture = None;
        // SAFETY: the transform is set up and `input` a filled sample.
        match unsafe { self.transform.ProcessInput(0, &input, 0) } {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                picture = self.drain(picture)?;
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, &input, 0) }
                    .map_err(call("IMFTransform::ProcessInput"))?;
            }
            Err(error) => return Err(call("IMFTransform::ProcessInput")(error)),
        }
        self.drain(picture)
    }

    /// Takes every picture out, keeping the last, and follows size changes.
    fn drain(&mut self, mut last: Option<Nv12Picture>) -> Result<Option<Nv12Picture>, Error> {
        loop {
            let output = match self.provides_samples {
                true => None,
                false => Some(sample(self.api, None, self.output_size)?),
            };
            let mut data = Vec::new();
            match take(&self.transform, output, &mut data) {
                Ok(None) => return Ok(last),
                Ok(Some(_)) => {
                    let needed = self.stride as usize * self.height as usize * 3 / 2;
                    if data.len() < needed {
                        return Err(Error::Picture("the decoder returned a short picture"));
                    }
                    last = Some(Nv12Picture {
                        data,
                        width: self.width,
                        height: self.height,
                        stride: self.stride,
                    });
                }
                Err(Error::Call { source, .. })
                    if source.code() == MF_E_TRANSFORM_STREAM_CHANGE =>
                {
                    self.choose_output()?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}
