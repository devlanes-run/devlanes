//! Socket options std and socket2 do not offer.

use std::io;
use std::os::windows::io::AsRawSocket;

use windows_sys::Win32::Networking::WinSock::{
    SIO_TCP_INITIAL_RTO, SO_EXCLUSIVEADDRUSE, SOCKET, SOCKET_ERROR, SOL_SOCKET,
    TCP_INITIAL_RTO_DEFAULT_RTT, TCP_INITIAL_RTO_PARAMETERS, WSAGetLastError, WSAIoctl, getsockopt,
    setsockopt,
};

/// Makes a connect on `socket`, which is not connected yet, fail as soon as the
/// peer refuses it. Windows otherwise answers a reset by sending the SYN again,
/// so a refused loopback connect takes about 2 s. A lost SYN is not resent
/// either, so use it only for loopback, where SYNs are not lost.
pub fn skip_syn_retransmissions(socket: &impl AsRawSocket) -> io::Result<()> {
    // `TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS`, which mstcpip.h defines as
    // `((UCHAR) -2)` for this u8 field; windows-sys types the constant as u16.
    const NO_SYN_RETRANSMISSIONS: u8 = 254;
    let parameters = TCP_INITIAL_RTO_PARAMETERS {
        Rtt: TCP_INITIAL_RTO_DEFAULT_RTT as u16,
        MaxSynRetransmissions: NO_SYN_RETRANSMISSIONS,
    };
    let mut returned: u32 = 0;
    // SAFETY: the socket is borrowed, so it stays open for the call. The input
    // points at a `TCP_INITIAL_RTO_PARAMETERS` of the length passed, which
    // WSAIoctl only reads; there is no output buffer, `returned` is writable, and
    // without an OVERLAPPED or completion routine the call completes before it returns.
    let result = unsafe {
        WSAIoctl(
            socket.as_raw_socket() as SOCKET,
            SIO_TCP_INITIAL_RTO,
            (&raw const parameters).cast(),
            size_of::<TCP_INITIAL_RTO_PARAMETERS>() as u32,
            std::ptr::null_mut(),
            0,
            &raw mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_socket_error());
    }
    Ok(())
}

/// Sets `SO_EXCLUSIVEADDRUSE` on a socket that is not bound yet, so no other
/// socket can bind its port while it is open, not even on a more specific
/// address. A port another socket holds then fails `bind` with `AddrInUse` or
/// `PermissionDenied`. Never combine it with `SO_REUSEADDR`.
pub fn set_exclusive_address_use(socket: &impl AsRawSocket) -> io::Result<()> {
    let enable: i32 = 1;
    // SAFETY: the socket is borrowed, so it stays open for the call; `optval`
    // points at an i32 of the length passed, which setsockopt only reads.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as SOCKET,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw const enable).cast(),
            size_of::<i32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_socket_error());
    }
    Ok(())
}

/// Whether `SO_EXCLUSIVEADDRUSE` is set on `socket`.
pub fn exclusive_address_use(socket: &impl AsRawSocket) -> io::Result<bool> {
    let mut value: i32 = 0;
    let mut length = size_of::<i32>() as i32;
    // SAFETY: the socket is borrowed, so it stays open for the call; `optval`
    // and `optlen` point at writable values of the sizes passed.
    let result = unsafe {
        getsockopt(
            socket.as_raw_socket() as SOCKET,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw mut value).cast(),
            &raw mut length,
        )
    };
    if result == SOCKET_ERROR {
        return Err(last_socket_error());
    }
    Ok(value != 0)
}

/// This thread's last Winsock error, which uses Win32 error numbers.
fn last_socket_error() -> io::Error {
    // SAFETY: no arguments; reads this thread's last Winsock error.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}
