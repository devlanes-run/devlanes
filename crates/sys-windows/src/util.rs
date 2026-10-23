//! Helpers shared by the FFI modules.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::core::HRESULT;

/// A NUL-terminated UTF-16 copy for Win32 string parameters. A NUL inside
/// `text` is refused: Win32 would silently stop reading there.
pub(crate) fn wide(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "string contains a NUL character",
        ));
    }
    units.push(0);
    Ok(units)
}

/// Takes ownership of a handle a Win32 call just returned, or the last error
/// when it is null.
pub(crate) fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the caller just received `handle` from the OS and owns it.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The raw handle for a Win32 call; ownership stays with `handle`.
pub(crate) fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle()
}

/// An error for a failed `HRESULT`. HRESULTs are not Win32 error codes, so
/// they never go through `io::Error::from_raw_os_error`.
pub(crate) fn hresult_error(result: HRESULT) -> io::Error {
    io::Error::other(format!("HRESULT {result:#x}"))
}

/// The UTF-16 units before the first NUL of a fixed-size field.
pub(crate) fn until_nul(field: &[u16]) -> &[u16] {
    let len = field
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(field.len());
    &field[..len]
}

/// The error `code` (a Win32 error) as an `io::Error`.
pub(crate) fn win32_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code.cast_signed())
}
