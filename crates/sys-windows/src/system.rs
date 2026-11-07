//! Facts about this computer.

use std::io;

use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
use windows_sys::Win32::System::SystemInformation::{
    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
};

/// This computer's host name, as `hostname.exe` prints it: the DNS host name
/// of the physical machine, not cut to 15 characters like the NetBIOS name in
/// `COMPUTERNAME`. Read from the system, so it does not depend on the
/// caller's environment.
pub fn computer_name() -> io::Result<String> {
    let mut buffer = vec![0_u16; 64];
    loop {
        let mut size =
            u32::try_from(buffer.len()).map_err(|_| io::Error::other("computer name too long"))?;
        // SAFETY: `buffer` holds `size` writable UTF-16 units and `size` is a
        // valid pointer for the call to update.
        let ok = unsafe {
            GetComputerNameExW(
                ComputerNamePhysicalDnsHostname,
                buffer.as_mut_ptr(),
                &raw mut size,
            )
        };
        if ok != 0 {
            // On success `size` is the length without the NUL.
            buffer.truncate(size as usize);
            return Ok(String::from_utf16_lossy(&buffer));
        }
        let error = io::Error::last_os_error();
        // On `ERROR_MORE_DATA` `size` is the length needed, NUL included.
        if error.raw_os_error() == Some(ERROR_MORE_DATA.cast_signed())
            && size as usize > buffer.len()
        {
            buffer.resize(size as usize, 0);
            continue;
        }
        return Err(error);
    }
}
