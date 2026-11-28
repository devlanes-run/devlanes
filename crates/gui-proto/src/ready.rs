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
//!
//! A host whose backend cannot run on the machine prints an [`Unready`] line
//! instead and exits with [`UNREADY_EXIT`]; [`HostLine::parse`] reads either.
//!
//! ```json
//! {"unready":1,"devlanes":"0.17.0","protocol":1,"reason":"no_desktop_session","message":"no desktop session"}
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// The variable that carries the control token from the daemon to the host
/// it starts; the daemon's control connection presents it. It keeps other
/// accounts on the machine, which can reach the loopback port, from taking
/// the control role, but not processes of the same account, which can read
/// the host's environment and can already do everything through the daemon.
/// It is an environment variable and never a flag, since other accounts can
/// read a process's arguments but not its environment.
pub const TOKEN_ENV: &str = "DEVLANES_GUI_TOKEN";

/// The format version in the `ready` field.
pub const READY_FORMAT: u32 = 1;

/// The longest ready line a reader should accept, newline excluded. Read at
/// most this many bytes (plus one) before calling [`Ready::parse`], so a host
/// that prints something else cannot make the reader buffer without end.
pub const MAX_READY_LINE: usize = 64 * 1024;

/// What a GUI host reports once it listens. It has no `Deserialize`:
/// [`Ready::parse`] is the only way to read one, so every reader gets its
/// checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Codecs the host lacks on this machine, such as H.264 without
    /// OpenH264; its viewers then get the next codec both ends have, `raw`
    /// at worst. Left out of the line when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codecs: Vec<CodecProblem>,
}

/// The most [`CodecProblem`]s a ready line may carry.
pub const MAX_CODEC_PROBLEMS: usize = 8;

/// A codec a GUI host or viewer machine lacks, and why: in the host's ready
/// line, and in `devlanes-gui codecs`, which `devlanes doctor` reads.
/// Clients word it; the fields are facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecProblem {
    /// The codec's name, such as `h264`.
    pub codec: String,
    /// Which side is missing: a host needs to encode, a viewer to decode.
    #[serde(default)]
    pub side: CodecSide,
    /// Why it is missing.
    pub reason: CodecReason,
    /// The library that is missing or refused, such as `OpenH264`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// The version found, for [`CodecReason::UnsupportedVersion`] and
    /// [`CodecReason::DecoderTooOld`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
    /// The versions devlanes uses on that side, such as `2.2 to 2.7`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<String>,
    /// The loader's or the OS's own message, for logs.
    #[serde(default)]
    pub detail: String,
}

/// Why a codec is missing. Unknown words read as [`CodecReason::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecReason {
    /// Its library is not installed.
    MissingLibrary,
    /// Its library is a version devlanes cannot use.
    UnsupportedVersion,
    /// Its library encodes, but its decoder has a known security flaw
    /// (OpenH264 before 2.6), so this machine does not decode with it.
    DecoderTooOld,
    /// The OS has no such codec, or it did not start.
    Unavailable,
    /// Anything else; `detail` says what.
    #[serde(other)]
    Other,
}

impl CodecReason {
    /// The word on the line, such as `missing_library`.
    pub fn word(self) -> &'static str {
        match self {
            Self::MissingLibrary => "missing_library",
            Self::UnsupportedVersion => "unsupported_version",
            Self::DecoderTooOld => "decoder_too_old",
            Self::Unavailable => "unavailable",
            Self::Other => "other",
        }
    }
}

/// Which side of a codec a [`CodecProblem`] is about. Unknown words read as
/// [`CodecSide::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecSide {
    /// Encoding, which a GUI host does.
    Encode,
    /// Decoding, which a viewer does.
    Decode,
    /// A side this build does not know, or none given.
    #[default]
    #[serde(other)]
    Other,
}

impl CodecSide {
    /// The word on the line, such as `encode`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Encode => "encode",
            Self::Decode => "decode",
            Self::Other => "other",
        }
    }
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
    /// A variable set twice, removed twice, or both set and removed.
    #[error("the GUI host names the variable {0:?} more than once")]
    Repeated(String),
    /// More than [`MAX_CODEC_PROBLEMS`] codec problems.
    #[error("the GUI host lists {0} codec problems, more than {MAX_CODEC_PROBLEMS}")]
    Codecs(usize),
}

/// The line as it is read, before the checks of [`Ready::parse`]. Unknown
/// fields are ignored; a field given twice fails.
#[derive(Deserialize)]
struct Wire {
    ready: u32,
    devlanes: String,
    protocol: u16,
    address: SocketAddr,
    set: Pairs,
    remove: Vec<String>,
    real_desktop: bool,
    /// Added after the first release of the format, so a line without it reads.
    #[serde(default)]
    codecs: Vec<CodecProblem>,
}

/// Just the format version, read before anything else.
#[derive(Deserialize)]
struct Format {
    ready: Option<u64>,
}

/// The `set` object as its entries in order, so a name given twice shows
/// instead of the last one silently winning.
struct Pairs(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Pairs {
    /// Collects every entry of a JSON object of strings.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads the entries of one map.
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = Pairs;

            /// What `set` must be.
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object of strings")
            }

            /// Every entry, in order.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Pairs, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, String>()? {
                    entries.push(entry);
                }
                Ok(Pairs(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
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
            codecs: Vec::new(),
        }
    }

    /// The line as the host prints it, without the newline.
    pub fn to_line(&self) -> String {
        // Strings, numbers, booleans and string maps always serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a line (without its newline) strictly: the format must be
    /// [`READY_FORMAT`], the address loopback with a port other than 0, every
    /// variable name non-empty without `=` or NUL and named once across `set`
    /// and `remove`, and values without NUL. Fields this build does not know
    /// are ignored. The caller reads the line with a cap of
    /// [`MAX_READY_LINE`] bytes, never with a line reader that buffers
    /// without bound.
    pub fn parse(line: &str) -> Result<Self, ReadyError> {
        if line.len() > MAX_READY_LINE {
            return Err(ReadyError::TooLong(line.len()));
        }
        // The format first, so a later format fails as such and not on a
        // field it renamed.
        let format: Format = serde_json::from_str(line)?;
        if let Some(format) = format.ready
            && format != u64::from(READY_FORMAT)
        {
            return Err(ReadyError::Format(format));
        }
        let wire: Wire = serde_json::from_str(line)?;
        if !wire.address.ip().is_loopback() || wire.address.port() == 0 {
            return Err(ReadyError::Address(wire.address));
        }
        let bad_name = |name: &str| name.is_empty() || name.contains(['=', '\0']);
        let mut set = BTreeMap::new();
        for (name, value) in wire.set.0 {
            if bad_name(&name) || value.contains('\0') {
                return Err(ReadyError::Variable(name));
            }
            if set.contains_key(&name) {
                return Err(ReadyError::Repeated(name));
            }
            set.insert(name, value);
        }
        let mut remove: Vec<String> = Vec::new();
        for name in wire.remove {
            if bad_name(&name) {
                return Err(ReadyError::Variable(name));
            }
            if set.contains_key(&name) || remove.contains(&name) {
                return Err(ReadyError::Repeated(name));
            }
            remove.push(name);
        }
        if wire.codecs.len() > MAX_CODEC_PROBLEMS {
            return Err(ReadyError::Codecs(wire.codecs.len()));
        }
        Ok(Self {
            ready: wire.ready,
            devlanes: wire.devlanes,
            protocol: wire.protocol,
            address: wire.address,
            set,
            remove,
            real_desktop: wire.real_desktop,
            codecs: wire.codecs,
        })
    }
}

/// The exit code of a host that printed an [`Unready`] line.
pub const UNREADY_EXIT: u8 = 3;

/// Why a host's backend cannot run on this machine. Unknown words read as
/// [`UnavailableReason::Other`], so a later host can add reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// No logged-in desktop session to show windows in.
    NoDesktopSession,
    /// The host runs elevated, so it cannot drive the user's windows.
    Elevated,
    /// A system library or program is missing.
    MissingLibrary,
    /// This OS has no backend.
    UnsupportedOs,
    /// Anything else; the message says what.
    #[serde(other)]
    Other,
}

impl UnavailableReason {
    /// The word on the line, such as `no_desktop_session`.
    pub fn word(self) -> &'static str {
        match self {
            Self::NoDesktopSession => "no_desktop_session",
            Self::Elevated => "elevated",
            Self::MissingLibrary => "missing_library",
            Self::UnsupportedOs => "unsupported_os",
            Self::Other => "other",
        }
    }
}

/// What a host prints instead of [`Ready`] when its backend cannot run here,
/// before it exits with [`UNREADY_EXIT`]. Read with [`HostLine::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unready {
    /// [`READY_FORMAT`], under this name so the two lines cannot be confused.
    pub unready: u32,
    /// The devlanes release the host belongs to.
    pub devlanes: String,
    /// The GUI protocol version it speaks.
    pub protocol: u16,
    /// What is missing.
    pub reason: UnavailableReason,
    /// The same for people, such as which library is missing.
    pub message: String,
}

impl Unready {
    /// The line for a backend that cannot run because of `reason`.
    pub fn new(
        devlanes: impl Into<String>,
        reason: UnavailableReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            unready: READY_FORMAT,
            devlanes: devlanes.into(),
            protocol: crate::PROTOCOL_VERSION,
            reason,
            message: message.into(),
        }
    }

    /// The line as the host prints it, without the newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// [`Unready`] as it is read.
#[derive(Deserialize)]
struct UnreadyWire {
    unready: u32,
    devlanes: String,
    protocol: u16,
    reason: UnavailableReason,
    message: String,
}

/// The first line a host prints: it listens, or its backend cannot run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostLine {
    /// It listens.
    Ready(Ready),
    /// It cannot run here and exits.
    Unready(Unready),
}

impl HostLine {
    /// Reads a line (without its newline): an [`Unready`] line when it has an
    /// `unready` field, else a [`Ready`] line with the checks of
    /// [`Ready::parse`]. The same length cap and format rule hold for both.
    pub fn parse(line: &str) -> Result<Self, ReadyError> {
        if line.len() > MAX_READY_LINE {
            return Err(ReadyError::TooLong(line.len()));
        }
        /// Just the field that tells the lines apart.
        #[derive(Deserialize)]
        struct Kind {
            unready: Option<u64>,
        }
        let kind: Kind = serde_json::from_str(line)?;
        match kind.unready {
            None => Ready::parse(line).map(Self::Ready),
            Some(format) if format != u64::from(READY_FORMAT) => Err(ReadyError::Format(format)),
            Some(_) => {
                let wire: UnreadyWire = serde_json::from_str(line)?;
                Ok(Self::Unready(Unready {
                    unready: wire.unready,
                    devlanes: wire.devlanes,
                    protocol: wire.protocol,
                    reason: wire.reason,
                    message: wire.message,
                }))
            }
        }
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

        // Codec problems read back; a reason this build does not know reads
        // as other, and more than the cap is refused.
        let mut lacking = ready.clone();
        lacking.codecs = vec![CodecProblem {
            codec: "h264".into(),
            side: CodecSide::Encode,
            reason: CodecReason::UnsupportedVersion,
            library: Some("OpenH264".into()),
            found: Some("3.0.0".into()),
            supported: Some("2.2 to 2.7".into()),
            detail: String::new(),
        }];
        let line = lacking.to_line();
        assert_eq!(Ready::parse(&line).expect("reads"), lacking);
        let mut value: serde_json::Value = serde_json::from_str(&line).expect("json");
        value["codecs"][0]["reason"] = serde_json::json!("gone_fishing");
        let read = Ready::parse(&value.to_string()).expect("reads");
        assert_eq!(read.codecs[0].reason, CodecReason::Other);
        value["codecs"] =
            serde_json::json!(vec![value["codecs"][0].clone(); MAX_CODEC_PROBLEMS + 1]);
        assert!(matches!(
            Ready::parse(&value.to_string()),
            Err(ReadyError::Codecs(_))
        ));
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
        let both = with(&|v| {
            v["set"] = serde_json::json!({ "A": "x" });
            v["remove"] = serde_json::json!(["A"]);
        });
        assert!(matches!(both, Err(ReadyError::Repeated(_))), "{both:?}");
        let twice = with(&|v| v["remove"] = serde_json::json!(["A", "A"]));
        assert!(matches!(twice, Err(ReadyError::Repeated(_))), "{twice:?}");
        // JSON values hold one entry per key, so this one is written out.
        let line = good
            .to_line()
            .replace(r#""set":{}"#, r#""set":{"A":"1","A":"2"}"#);
        assert!(line.contains(r#""A":"2""#), "{line}");
        let duplicate = Ready::parse(&line);
        assert!(
            matches!(duplicate, Err(ReadyError::Repeated(_))),
            "{duplicate:?}"
        );
        let long = " ".repeat(MAX_READY_LINE + 1);
        assert!(matches!(Ready::parse(&long), Err(ReadyError::TooLong(_))));
    }

    #[test]
    fn a_host_line_is_either_line_and_unknown_reasons_read_as_other() {
        let ready = Ready::new(address(), "0.17.0", &[], false);
        assert_eq!(
            HostLine::parse(&ready.to_line()).expect("ready"),
            HostLine::Ready(ready)
        );
        let unready = Unready::new("0.17.0", UnavailableReason::MissingLibrary, "no pixman");
        assert_eq!(
            HostLine::parse(&unready.to_line()).expect("unready"),
            HostLine::Unready(unready.clone())
        );
        let later = unready
            .to_line()
            .replace("missing_library", "needs_a_reboot");
        match HostLine::parse(&later).expect("later reason") {
            HostLine::Unready(line) => assert_eq!(line.reason, UnavailableReason::Other),
            other => panic!("{other:?}"),
        }
        let format = unready
            .to_line()
            .replace(r#""unready":1"#, r#""unready":2"#);
        assert!(matches!(
            HostLine::parse(&format),
            Err(ReadyError::Format(2))
        ));
    }
}
