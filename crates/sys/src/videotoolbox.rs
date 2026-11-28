//! The VideoToolbox H.264 decoder of macOS, for the GUI viewer: an Annex B
//! byte stream in, NV12 pictures out, one access unit at a time.
//!
//! VideoToolbox takes length-prefixed NAL units and a format description
//! made from the stream's parameter sets, so each access unit is split at
//! its start codes: SPS and PPS units (re)build the format description, and
//! the slices go to the session with 4-byte lengths. Frames are decoded
//! synchronously: the call waits for the session's output before it returns.
//!
//! Written against Apple's documented C API (CoreFoundation, CoreMedia,
//! CoreVideo, VideoToolbox) without bindings crates.

use std::ffi::c_void;
use std::sync::Mutex;

type OsStatus = i32;
type CfTypeRef = *const c_void;
type CfAllocatorRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfStringRef = *const c_void;
type CfNumberRef = *const c_void;
type CmFormatDescriptionRef = *const c_void;
type CmBlockBufferRef = *const c_void;
type CmSampleBufferRef = *const c_void;
type CvImageBufferRef = *const c_void;
type VtDecompressionSessionRef = *const c_void;

/// `CMTime`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

/// `CFDictionaryKeyCallBacks` and `CFDictionaryValueCallBacks`, only ever
/// passed by address.
#[repr(C)]
struct CfCallBacks {
    _private: [u8; 0],
}

/// `VTDecompressionOutputCallbackRecord`.
#[repr(C)]
struct OutputCallbackRecord {
    callback: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        OsStatus,
        u32,
        CvImageBufferRef,
        CmTime,
        CmTime,
    ),
    refcon: *mut c_void,
}

const K_CF_NUMBER_SINT32_TYPE: isize = 3;
/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`, `'420v'`.
const PIXEL_FORMAT_NV12_VIDEO_RANGE: i32 = 0x3432_3076;
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorNull: CfAllocatorRef;
    static kCFTypeDictionaryKeyCallBacks: CfCallBacks;
    static kCFTypeDictionaryValueCallBacks: CfCallBacks;
    fn CFRelease(cf: CfTypeRef);
    fn CFRetain(cf: CfTypeRef) -> CfTypeRef;
    fn CFNumberCreate(allocator: CfAllocatorRef, kind: isize, value: *const c_void) -> CfNumberRef;
    fn CFDictionaryCreate(
        allocator: CfAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const CfCallBacks,
        value_callbacks: *const CfCallBacks,
    ) -> CfDictionaryRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CfAllocatorRef,
        parameter_set_count: usize,
        parameter_set_pointers: *const *const u8,
        parameter_set_sizes: *const usize,
        nal_unit_header_length: i32,
        format_description_out: *mut CmFormatDescriptionRef,
    ) -> OsStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: CfAllocatorRef,
        memory_block: *mut c_void,
        block_length: usize,
        block_allocator: CfAllocatorRef,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        block_buffer_out: *mut CmBlockBufferRef,
    ) -> OsStatus;
    fn CMSampleBufferCreateReady(
        allocator: CfAllocatorRef,
        data_buffer: CmBlockBufferRef,
        format_description: CmFormatDescriptionRef,
        num_samples: isize,
        num_sample_timing_entries: isize,
        sample_timing_array: *const c_void,
        num_sample_size_entries: isize,
        sample_size_array: *const usize,
        sample_buffer_out: *mut CmSampleBufferRef,
    ) -> OsStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CfStringRef;
    fn CVPixelBufferLockBaseAddress(buffer: CvImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: CvImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetPixelFormatType(buffer: CvImageBufferRef) -> u32;
    fn CVPixelBufferGetWidth(buffer: CvImageBufferRef) -> usize;
    fn CVPixelBufferGetHeight(buffer: CvImageBufferRef) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: CvImageBufferRef, plane: usize) -> *const u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: CvImageBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(buffer: CvImageBufferRef, plane: usize) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: CfAllocatorRef,
        format_description: CmFormatDescriptionRef,
        decoder_specification: CfDictionaryRef,
        destination_image_buffer_attributes: CfDictionaryRef,
        output_callback: *const OutputCallbackRecord,
        session_out: *mut VtDecompressionSessionRef,
    ) -> OsStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: VtDecompressionSessionRef,
        sample_buffer: CmSampleBufferRef,
        decode_flags: u32,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OsStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(
        session: VtDecompressionSessionRef,
    ) -> OsStatus;
    fn VTDecompressionSessionCanAcceptFormatDescription(
        session: VtDecompressionSessionRef,
        format_description: CmFormatDescriptionRef,
    ) -> u8;
    fn VTDecompressionSessionInvalidate(session: VtDecompressionSessionRef);
}

/// Why a decode failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A call returned an error status.
    #[error("VideoToolbox {call} failed with status {status}")]
    Call {
        /// The call.
        call: &'static str,
        /// What it returned.
        status: i32,
    },
    /// Slices came before any parameter sets.
    #[error("the stream has no parameter sets yet")]
    NoParameterSets,
    /// The output is not the NV12 picture asked for.
    #[error("VideoToolbox returned an unexpected picture")]
    Picture,
}

/// `Ok` for a zero status.
fn check(call: &'static str, status: OsStatus) -> Result<(), Error> {
    match status {
        0 => Ok(()),
        status => Err(Error::Call { call, status }),
    }
}

/// An owned CoreFoundation reference, released on drop.
struct Owned(CfTypeRef);

impl Drop for Owned {
    /// Releases the reference.
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the reference was created or retained for this owner
            // and is released once.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// A decoded NV12 picture, its planes copied out of VideoToolbox's buffer.
#[derive(Debug, Clone)]
pub struct Nv12Picture {
    /// Luma, `y_stride` bytes a row, `height` rows.
    pub y: Vec<u8>,
    /// Interleaved chroma, `uv_stride` bytes a row, `height / 2` rows rounded up.
    pub uv: Vec<u8>,
    /// Bytes between luma rows.
    pub y_stride: usize,
    /// Bytes between chroma rows.
    pub uv_stride: usize,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Where the output callback leaves the decoded picture.
#[derive(Default)]
struct Slot {
    status: i32,
    /// A retained image buffer.
    image: Option<usize>,
}

/// The session's output callback: keeps the image buffer for [`Decoder::decode`].
unsafe extern "C" fn on_output(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OsStatus,
    _flags: u32,
    image: CvImageBufferRef,
    _time: CmTime,
    _duration: CmTime,
) {
    // SAFETY: `refcon` is the decoder's boxed slot, alive for the session.
    let slot = unsafe { &*(refcon as *const Mutex<Slot>) };
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.status = status;
    if status == 0 && !image.is_null() {
        // SAFETY: the buffer is valid during the callback; retaining keeps it
        // for the decoder, which releases it.
        let image = unsafe { CFRetain(image) };
        if let Some(old) = slot.image.replace(image as usize) {
            // SAFETY: a buffer an earlier callback retained and nobody took.
            unsafe { CFRelease(old as CfTypeRef) };
        }
    }
}

/// A VideoToolbox decoder of one H.264 stream.
pub struct Decoder {
    session: Option<Owned>,
    format: Option<Owned>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    slot: Box<Mutex<Slot>>,
}

// SAFETY: a decompression session may be used from any thread, one call at
// a time, which `&mut self` ensures; the slot it writes is behind a mutex.
unsafe impl Send for Decoder {}

impl std::fmt::Debug for Decoder {
    /// Whether a session runs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("session", &self.session.is_some())
            .finish_non_exhaustive()
    }
}

/// The NAL units of an Annex B byte stream, without their start codes.
fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(stream.len(), |next| next - 3);
        // A four-byte start code leaves a zero before the next three-byte one.
        while end > start && stream[end - 1] == 0 && n + 1 < starts.len() {
            end -= 1;
        }
        if end > start {
            units.push(&stream[start..end]);
        }
    }
    units
}

impl Decoder {
    /// A decoder that starts its session on the first parameter sets.
    pub fn new() -> Self {
        Self {
            session: None,
            format: None,
            sps: Vec::new(),
            pps: Vec::new(),
            slot: Box::new(Mutex::new(Slot::default())),
        }
    }

    /// Rebuilds the format description from the current parameter sets, and
    /// the session when it cannot take the new description (a new size).
    fn refresh(&mut self) -> Result<(), Error> {
        let pointers = [self.sps.as_ptr(), self.pps.as_ptr()];
        let sizes = [self.sps.len(), self.pps.len()];
        let mut format: CmFormatDescriptionRef = std::ptr::null();
        // SAFETY: two parameter sets with their sizes; the description copies them.
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                std::ptr::null(),
                2,
                pointers.as_ptr(),
                sizes.as_ptr(),
                4,
                &mut format,
            )
        };
        check(
            "CMVideoFormatDescriptionCreateFromH264ParameterSets",
            status,
        )?;
        let format = Owned(format);
        let keeps = self.session.as_ref().is_some_and(|session| {
            // SAFETY: a live session and a valid description.
            unsafe { VTDecompressionSessionCanAcceptFormatDescription(session.0, format.0) != 0 }
        });
        if !keeps {
            self.close();
            self.session = Some(self.open(&format)?);
        }
        self.format = Some(format);
        Ok(())
    }

    /// A session for `format` with NV12 output.
    fn open(&self, format: &Owned) -> Result<Owned, Error> {
        let pixel_format = PIXEL_FORMAT_NV12_VIDEO_RANGE;
        // SAFETY: a dictionary of one CFNumber under CoreVideo's key, built
        // with the CF type callbacks, so it retains both; both are released
        // here after the session copied what it needs.
        unsafe {
            let number = Owned(CFNumberCreate(
                std::ptr::null(),
                K_CF_NUMBER_SINT32_TYPE,
                (&pixel_format as *const i32).cast(),
            ));
            let keys = [kCVPixelBufferPixelFormatTypeKey];
            let values = [number.0];
            let attributes = Owned(CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            ));
            let callback = OutputCallbackRecord {
                callback: on_output,
                refcon: (&*self.slot as *const Mutex<Slot>).cast_mut().cast(),
            };
            let mut session: VtDecompressionSessionRef = std::ptr::null();
            let status = VTDecompressionSessionCreate(
                std::ptr::null(),
                format.0,
                std::ptr::null(),
                attributes.0,
                &callback,
                &mut session,
            );
            check("VTDecompressionSessionCreate", status)?;
            Ok(Owned(session))
        }
    }

    /// Invalidates and releases the session.
    fn close(&mut self) {
        if let Some(session) = self.session.take() {
            // SAFETY: a live session, not used after this.
            unsafe { VTDecompressionSessionInvalidate(session.0) };
        }
    }

    /// Decodes one access unit and returns its picture, or `None` when it
    /// held no slice (parameter sets only).
    pub fn decode(&mut self, access_unit: &[u8]) -> Result<Option<Nv12Picture>, Error> {
        let mut slices = Vec::new();
        let mut changed = false;
        for unit in nal_units(access_unit) {
            match unit[0] & 0x1f {
                7 if unit != self.sps.as_slice() => {
                    self.sps = unit.to_vec();
                    changed = true;
                }
                8 if unit != self.pps.as_slice() => {
                    self.pps = unit.to_vec();
                    changed = true;
                }
                // Slices; delimiters, SEI and the rest are not needed.
                1..=5 => {
                    slices.extend_from_slice(&(unit.len() as u32).to_be_bytes());
                    slices.extend_from_slice(unit);
                }
                _ => {}
            }
        }
        if changed && !self.sps.is_empty() && !self.pps.is_empty() {
            self.refresh()?;
        }
        if slices.is_empty() {
            return Ok(None);
        }
        let (Some(session), Some(format)) = (&self.session, &self.format) else {
            return Err(Error::NoParameterSets);
        };
        let mut block: CmBlockBufferRef = std::ptr::null();
        let mut sample: CmSampleBufferRef = std::ptr::null();
        let sizes = [slices.len()];
        // SAFETY: the block buffer points at `slices` without owning it
        // (kCFAllocatorNull), and both buffers are released before `slices`
        // goes, since the decode is synchronous and waited for.
        unsafe {
            let status = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                slices.as_mut_ptr().cast(),
                slices.len(),
                kCFAllocatorNull,
                std::ptr::null(),
                0,
                slices.len(),
                0,
                &mut block,
            );
            check("CMBlockBufferCreateWithMemoryBlock", status)?;
            let block = Owned(block);
            let status = CMSampleBufferCreateReady(
                std::ptr::null(),
                block.0,
                format.0,
                1,
                0,
                std::ptr::null(),
                1,
                sizes.as_ptr(),
                &mut sample,
            );
            check("CMSampleBufferCreateReady", status)?;
            let sample = Owned(sample);
            let mut flags = 0u32;
            let status = VTDecompressionSessionDecodeFrame(
                session.0,
                sample.0,
                0,
                std::ptr::null_mut(),
                &mut flags,
            );
            check("VTDecompressionSessionDecodeFrame", status)?;
            let status = VTDecompressionSessionWaitForAsynchronousFrames(session.0);
            check("VTDecompressionSessionWaitForAsynchronousFrames", status)?;
        }
        let (status, image) = {
            let mut slot = self
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (std::mem::take(&mut slot.status), slot.image.take())
        };
        check("the decoder's output", status)?;
        let Some(image) = image else {
            return Ok(None);
        };
        let image = Owned(image as CfTypeRef);
        copy_planes(image.0).map(Some)
    }
}

impl Default for Decoder {
    /// [`Decoder::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Decoder {
    /// Ends the session and releases a picture nobody took.
    fn drop(&mut self) {
        self.close();
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(image) = slot.image.take() {
            // SAFETY: a buffer the callback retained.
            unsafe { CFRelease(image as CfTypeRef) };
        }
    }
}

/// Copies the two planes of an NV12 pixel buffer.
fn copy_planes(image: CvImageBufferRef) -> Result<Nv12Picture, Error> {
    // SAFETY: the buffer is locked for reading while its planes are copied,
    // each for its own rows and bytes per row.
    unsafe {
        if CVPixelBufferGetPixelFormatType(image) != PIXEL_FORMAT_NV12_VIDEO_RANGE as u32 {
            return Err(Error::Picture);
        }
        check(
            "CVPixelBufferLockBaseAddress",
            CVPixelBufferLockBaseAddress(image, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY),
        )?;
        let plane = |index: usize| {
            let base = CVPixelBufferGetBaseAddressOfPlane(image, index);
            let stride = CVPixelBufferGetBytesPerRowOfPlane(image, index);
            let rows = CVPixelBufferGetHeightOfPlane(image, index);
            match base.is_null() {
                true => None,
                false => Some((
                    std::slice::from_raw_parts(base, stride * rows).to_vec(),
                    stride,
                )),
            }
        };
        let planes = (plane(0), plane(1));
        let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        CVPixelBufferUnlockBaseAddress(image, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
        let (Some((y, y_stride)), Some((uv, uv_stride))) = planes else {
            return Err(Error::Picture);
        };
        Ok(Nv12Picture {
            y,
            uv,
            y_stride,
            uv_stride,
            width: width as u32,
            height: height as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Start codes of three and four bytes both split units, and the zero
    /// of a four-byte one does not end up in the unit before it.
    #[test]
    fn annex_b_is_split_at_start_codes() {
        let stream = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5,
        ];
        let units = nal_units(&stream);
        assert_eq!(units, vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 4, 5]]);
    }
}
