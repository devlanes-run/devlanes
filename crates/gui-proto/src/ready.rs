//! The ready line: one JSON object a GUI host (`devlanes-gui host`) prints on
//! standard output once it listens, which the daemon reads before it starts
//! the unit. Behind the `serde` feature, so the protocol stays serde-free for
//! everything else.
//!
//! ```json
//! {"ready":1,"devlanes":"0.17.0","protocol":1,"address":"127.0.0.1:41234","set":{"WAYLAND_DISPLAY":"devlanes-41234"},"remove":["DISPLAY"],"real_desktop":false}
//! ```
//!
//! `ready` is the format version, [`READY_FORMAT`]. Fields added later keep
//! it, so [`Ready::parse`] ignores fields it does not know and refuses any
//! other format version.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// The format version in the `ready` field.
pub const READY_FORMAT: u32 = 1;

/// The longest ready line a reader should accept, newline excluded. Read at
/// most this many bytes (plus one) before calling [`Ready::parse`], so a host
/// that prints something else cannot make the reader buffer without end.
pub const MAX_READY_LINE: usize = 64 * 1024;

/// What a GUI host reports once it listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// [`READY_FORMAT`].
    pub ready: u32,
    /// The devlanes release the host belongs to, such as `0.17.0`.
    pub devlanes: String,
    /// The GUI protocol version it speaks, [`crate::PROTOCOL_VERSION`].
    pub protocol: u16,
    /// Where it listens: a loopback address with a port.
    pub address: SocketAddr,
    /// Variables the unit gets.
    pub set: BTreeMap<String, String>,
    /// Variables the unit must not inherit.
    pub remove: Vec<String>,
    /// Whether the display is the machine's real desktop, so a local viewer
    /// is not needed.
    pub real_desktop: bool,
}

/// Why [`Ready::parse`] refused a line.
#[derive(Debug, thiserror::Error)]
pub enum ReadyError {
    /// Longer than [`MAX_READY_LINE`].
    #[error("the GUI host's ready line is {0} bytes, more than {MAX_READY_LINE}")]
    TooLong(usize),
    /// Not a JSON object with the known fields.
    #[error("the GUI host's ready line does not read")]
    Json(#[from] serde_json::Error),
    /// Another format version.
    #[error("the GUI host's ready line has format {0}, expected {READY_FORMAT}")]
    Format(u64),
    /// The address is not loopback or has no port.
    #[error("the GUI host listens on {0}, not a loopback address with a port")]
    Address(SocketAddr),
    /// A variable name or value the environment cannot hold.
    #[error("the GUI host names an invalid variable {0:?}")]
    Variable(String),
}

impl Ready {
    /// The line for a host listening on `address`, from the backend's
    /// variables in order, where `None` removes one. A name given twice keeps
    /// its last entry: a value after a removal cancels the removal, and a
    /// removal after a value drops the value.
    pub fn new(
        address: SocketAddr,
        devlanes: impl Into<String>,
        env: &[(String, Option<String>)],
        real_desktop: bool,
    ) -> Self {
        let mut set = BTreeMap::new();
        let mut remove: Vec<String> = Vec::new();
        for (name, value) in env {
            match value {
                Some(value) => {
                    remove.retain(|removed| removed != name);
                    set.insert(name.clone(), value.clone());
                }
                None => {
                    set.remove(name);
                    if !remove.contains(name) {
                        remove.push(name.clone());
                    }
                }
            }
        }
        Self {
            ready: READY_FORMAT,
            devlanes: devlanes.into(),
            protocol: crate::PROTOCOL_VERSION,
            address,
            set,
            remove,
            real_desktop,
        }
    }

    /// The line as the host prints it, without the newline.
    pub fn to_line(&self) -> String {
        // Strings, numbers, booleans and string maps always serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a line (without its newline) strictly: the format must be
    /// [`READY_FORMAT`], the address loopback with a port other than 0, and
    /// every variable name non-empty without `=` or NUL, and values without
    /// NUL. Fields this build does not know are ignored.
    pub fn parse(line: &str) -> Result<Self, ReadyError> {
        if line.len() > MAX_READY_LINE {
            return Err(ReadyError::TooLong(line.len()));
        }
        let value: serde_json::Value = serde_json::from_str(line)?;
        // The format first, so a later format fails as such and not on a
        // field it renamed.
        if let Some(format) = value.get("ready").and_then(serde_json::Value::as_u64)
            && format != u64::from(READY_FORMAT)
        {
            return Err(ReadyError::Format(format));
        }
        let ready: Self = serde_json::from_value(value)?;
        if !ready.address.ip().is_loopback() || ready.address.port() == 0 {
            return Err(ReadyError::Address(ready.address));
        }
        let bad_name = |name: &str| name.is_empty() || name.contains(['=', '\0']);
        for (name, value) in &ready.set {
            if bad_name(name) || value.contains('\0') {
                return Err(ReadyError::Variable(name.clone()));
            }
        }
        if let Some(name) = ready.remove.iter().find(|name| bad_name(name)) {
            return Err(ReadyError::Variable(name.clone()));
        }
        Ok(ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listening address.
    fn address() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 41234))
    }

    /// The backend's variables for the table below.
    fn env(entries: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn the_last_entry_for_a_name_wins() {
        for (entries, set, remove) in [
            (
                vec![("A", Some("1")), ("A", Some("2"))],
                vec![("A", "2")],
                vec![],
            ),
            (vec![("A", Some("1")), ("A", None)], vec![], vec!["A"]),
            (
                vec![("A", None), ("A", Some("1"))],
                vec![("A", "1")],
                vec![],
            ),
            (
                vec![("D", None), ("W", Some("w")), ("D", None)],
                vec![("W", "w")],
                vec!["D"],
            ),
        ] {
            let ready = Ready::new(address(), "0.17.0", &env(&entries), false);
            let set: BTreeMap<String, String> = set
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
            assert_eq!(ready.set, set, "{entries:?}");
            assert_eq!(ready.remove, remove, "{entries:?}");
        }
    }

    #[test]
    fn a_line_reads_back_and_ignores_new_fields() {
        let ready = Ready::new(
            address(),
            "0.17.0",
            &env(&[("WAYLAND_DISPLAY", Some("devlanes-1")), ("DISPLAY", None)]),
            true,
        );
        let line = ready.to_line();
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(Ready::parse(&line).expect("reads"), ready);
        let mut value: serde_json::Value = serde_json::from_str(&line).expect("json");
        value["later"] = serde_json::json!({"field": 1});
        assert_eq!(Ready::parse(&value.to_string()).expect("reads"), ready);
    }

    #[test]
    fn the_reader_refuses_what_a_unit_cannot_use() {
        let good = Ready::new(address(), "0.17.0", &[], false);
        let with = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value = serde_json::to_value(&good).expect("json");
            change(&mut value);
            Ready::parse(&value.to_string())
        };
        let format = with(&|v| v["ready"] = 2.into());
        assert!(matches!(format, Err(ReadyError::Format(2))), "{format:?}");
        for address in [
            "0.0.0.0:41234",
            "192.0.2.1:41234",
            "127.0.0.1:0",
            "[::]:41234",
        ] {
            let refused = with(&|v| v["address"] = address.into());
            assert!(
                matches!(refused, Err(ReadyError::Address(_))),
                "{address}: {refused:?}"
            );
        }
        assert!(with(&|v| v["address"] = "[::1]:41234".into()).is_ok());
        for name in ["", "A=B", "A\0"] {
            let set = with(&|v| v["set"] = serde_json::json!({ name: "x" }));
            assert!(
                matches!(set, Err(ReadyError::Variable(_))),
                "{name:?}: {set:?}"
            );
            let remove = with(&|v| v["remove"] = serde_json::json!([name]));
            assert!(
                matches!(remove, Err(ReadyError::Variable(_))),
                "{name:?}: {remove:?}"
            );
        }
        let value = with(&|v| v["set"] = serde_json::json!({ "A": "x\0" }));
        assert!(matches!(value, Err(ReadyError::Variable(_))), "{value:?}");
        let missing = with(&|v| {
            v.as_object_mut().map(|object| object.remove("address"));
        });
        assert!(matches!(missing, Err(ReadyError::Json(_))), "{missing:?}");
        let long = " ".repeat(MAX_READY_LINE + 1);
        assert!(matches!(Ready::parse(&long), Err(ReadyError::TooLong(_))));
    }
}
