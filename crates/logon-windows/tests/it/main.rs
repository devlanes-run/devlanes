//! Install, status, Task Manager's off switch and uninstall against scratch
//! keys under `HKCU\Software`; the real Run key is never touched.
#![cfg(windows)]

use std::{io, path::Path};

use devlanes_logon_windows::{Change, Installed, Logon, Status, command};
use devlanes_sys_windows::registry;

/// A scratch `Run` and `StartupApproved` pair, deleted with everything under
/// it when dropped, pass or fail.
struct Scratch {
    root: String,
    logon: Logon,
}

impl Scratch {
    /// Keys named after this process and the clock, so parallel and earlier
    /// runs never share them.
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let root = format!(r"Software\devlanes-test-{}-{nanos}", std::process::id());
        let logon = Logon::at(
            format!(r"{root}\Run"),
            format!(r"{root}\Approved"),
            "devlanes",
        );
        Self { root, logon }
    }

    /// The raw Task Manager record.
    fn approved(&self) -> io::Result<Option<Vec<u8>>> {
        registry::read_binary(&format!(r"{}\Approved", self.root), "devlanes")
    }

    /// Writes a Task Manager record, as its Startup apps page does.
    fn set_approved(&self, record: &[u8]) -> io::Result<()> {
        registry::write_binary(&format!(r"{}\Approved", self.root), "devlanes", record)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = registry::delete_tree(&self.root);
    }
}

/// A Task Manager record that turns the entry off, with the time it did.
const DISABLED: [u8; 12] = [3, 0, 0, 0, 0x2c, 0xf1, 0x65, 0x7f, 0x99, 0xed, 0xd9, 0x01];

/// The full cycle: nothing, installed, up to date, updated for another
/// binary, uninstalled with Task Manager's record, then nothing to remove.
#[test]
fn install_status_uninstall() -> io::Result<()> {
    let scratch = Scratch::new();
    let logon = &scratch.logon;
    let exe = Path::new(r"C:\Program Files\acme tools\devlanes.exe");
    assert_eq!(
        logon.status()?,
        Status {
            command: None,
            enabled: true
        }
    );
    assert_eq!(
        logon.install(exe)?,
        Installed {
            change: Change::Installed,
            re_enabled: false
        }
    );
    let expected = r#""C:\Program Files\acme tools\devlanes.exe" daemon run"#;
    assert_eq!(
        logon.status()?,
        Status {
            command: Some(expected.into()),
            enabled: true
        }
    );
    assert_eq!(logon.install(exe)?.change, Change::UpToDate);
    let other = Path::new(r"D:\tools\devlanes.exe");
    assert_eq!(logon.install(other)?.change, Change::Updated);
    assert_eq!(
        logon.status()?.command.as_deref(),
        Some(command(other)?.as_str())
    );
    // An enabled record is left alone and does not count as turned off.
    scratch.set_approved(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
    assert!(logon.status()?.enabled);
    assert!(logon.uninstall()?);
    assert_eq!(scratch.approved()?, None);
    assert_eq!(logon.status()?.command, None);
    assert!(!logon.uninstall()?);
    Ok(())
}

/// Task Manager's off switch (an odd first byte) shows in the status, and
/// installing again turns it back on without rewriting the Run value.
#[test]
fn install_turns_a_disabled_entry_back_on() -> io::Result<()> {
    let scratch = Scratch::new();
    let logon = &scratch.logon;
    let exe = Path::new(r"C:\tools\devlanes.exe");
    logon.install(exe)?;
    scratch.set_approved(&DISABLED)?;
    assert!(!logon.status()?.enabled);
    // Any odd first byte is off, not only Task Manager's usual `03`.
    let mut also_off = DISABLED;
    also_off[0] = 7;
    scratch.set_approved(&also_off)?;
    assert!(!logon.status()?.enabled);
    assert_eq!(
        logon.install(exe)?,
        Installed {
            change: Change::UpToDate,
            re_enabled: true
        }
    );
    let status = logon.status()?;
    assert!(status.enabled, "{status:?}");
    assert_eq!(
        scratch.approved()?.and_then(|r| r.first().copied()),
        Some(2)
    );
    Ok(())
}

/// The location names the value under `HKCU`.
#[test]
fn location_names_the_value() {
    assert_eq!(
        Logon::user().location(),
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\devlanes"
    );
}
