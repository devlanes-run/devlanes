//! The UDP ports the endpoint bound, kept next to the key file so the next
//! start binds the same ones.
//!
//! A client keeps the box's socket addresses from the ticket. Without a
//! relay those are all it can dial, so a box that came back on other ports
//! would be unreachable until it is paired again. A stable port also keeps a
//! router's UPnP or NAT-PMP mapping stable, since iroh's port mapper maps the
//! bound IPv4 port.
//!
//! The file is not secret. Someone who can write the devlanes home can do far
//! worse than pick these ports, and ports below 1024 and 0 are ignored anyway,
//! so it cannot make the daemon try a privileged port.
//! Everything here blocks, so callers run it in `spawn_blocking`.

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The file's name, in the key file's directory.
const FILE: &str = "p2p-port";

/// The lowest port a saved value may name: below it are the privileged ports.
const LOWEST: u16 = 1024;

/// The ports of the endpoint's IPv4 and IPv6 sockets, each when known.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Ports {
    pub(crate) v4: Option<u16>,
    pub(crate) v6: Option<u16>,
}

impl Ports {
    /// The ports of `sockets`, the endpoint's bound sockets.
    pub(crate) fn of(sockets: &[SocketAddr]) -> Self {
        let port = |v4: bool| {
            sockets
                .iter()
                .find(|socket| socket.is_ipv4() == v4)
                .map(SocketAddr::port)
                .filter(|port| *port >= LOWEST)
        };
        Self {
            v4: port(true),
            v6: port(false),
        }
    }

    /// Whether no port is known.
    pub(crate) fn is_empty(&self) -> bool {
        self.v4.is_none() && self.v6.is_none()
    }
}

/// Where the ports of the endpoint whose key is `key_file` are kept.
pub(crate) fn path(key_file: &Path) -> PathBuf {
    key_file.with_file_name(FILE)
}

/// The saved ports, or none when the file is missing, unreadable or holds
/// something else: the endpoint then binds any port, and never fails for it.
pub(crate) fn load(path: &Path) -> Ports {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::debug!(%error, path = %path.display(), "ignoring the saved peer-to-peer ports");
            }
            Ports::default()
        }
    }
}

/// Reads `v4 <port>` and `v6 <port>` lines; anything else, and a port below
/// [`LOWEST`], is left out.
fn parse(text: &str) -> Ports {
    let mut ports = Ports::default();
    for line in text.lines() {
        let Some((family, port)) = line.trim().split_once(' ') else {
            continue;
        };
        let Some(port) = port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= LOWEST)
        else {
            continue;
        };
        match family {
            "v4" => ports.v4 = Some(port),
            "v6" => ports.v6 = Some(port),
            _ => {}
        }
    }
    ports
}

/// Saves `ports` through a temporary file renamed over the old one, so a
/// daemon starting at the same time reads either file whole.
pub(crate) fn save(path: &Path, ports: Ports) -> io::Result<()> {
    let mut text = String::new();
    if let Some(port) = ports.v4 {
        text.push_str(&format!("v4 {port}\n"));
    }
    if let Some(port) = ports.v6 {
        text.push_str(&format!("v6 {port}\n"));
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ports a file names reach the next bind only when they are real,
    /// unprivileged ports of a known family; anything else binds any port.
    #[test]
    fn only_unprivileged_ports_of_a_known_family_are_read() {
        let both = Ports {
            v4: Some(40_001),
            v6: Some(40_003),
        };
        let cases = [
            ("v4 40001\nv6 40003\n", both),
            ("  v6 40003\r\nv4 40001", both),
            (
                "v4 40001\n",
                Ports {
                    v4: Some(40_001),
                    v6: None,
                },
            ),
            ("v4 80\nv6 0\n", Ports::default()),
            ("v4 70000\nv5 40001\nv6 port\n", Ports::default()),
            ("\u{0}garbage", Ports::default()),
            ("", Ports::default()),
        ];
        for (text, ports) in cases {
            assert_eq!(parse(text), ports, "{text:?}");
        }
    }
}
