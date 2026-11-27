//! The command line Chrome starts a native host with.

use std::ffi::{OsStr, OsString};

/// How Chrome names the calling extension in `argv[1]`.
pub const SCHEME: &str = "chrome-extension://";

/// The ids of the extensions allowed to start the host. The Chrome Web Store
/// gave the devlanes extension this id. The extension's manifest carries the
/// store's public key as `key`, so unpacked builds get the same id.
pub const ALLOWED_IDS: &[&str] = &["konofbnhgejkgkonkleipkkpkphbfgee"];

/// The origins of [`ALLOWED_IDS`], as a host manifest's `allowed_origins`
/// lists them: `chrome-extension://<id>/`.
pub fn allowed_origins() -> Vec<String> {
    ALLOWED_IDS
        .iter()
        .map(|id| format!("{SCHEME}{id}/"))
        .collect()
}

/// An argument list the host refuses to run with.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error(
        "the browser host needs the caller's origin, such as {SCHEME}<id>/, as its first argument"
    )]
    Missing,
    #[error("the browser host refuses the origin {0}. It answers only the devlanes extension.")]
    Refused(String),
    #[error("the browser host doesn't take the argument {0}. Only the browser starts it.")]
    UnknownArgument(String),
}

/// Whether `arg`, the binary's first argument, is a Chrome extension origin,
/// which means Chrome started the binary as a native host.
pub fn is_origin(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|arg| arg.starts_with(SCHEME))
}

/// The allowed extension id `args` names, from the arguments after the program
/// name. Chrome on Windows adds `--parent-window=<handle>` after the origin,
/// and the host has no window to parent.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<&'static str, OriginError> {
    let mut args = args.into_iter();
    let origin = args.next().ok_or(OriginError::Missing)?;
    let origin = origin
        .into_string()
        .map_err(|origin| OriginError::Refused(origin.to_string_lossy().into_owned()))?;
    let Some(id) = origin
        .strip_prefix(SCHEME)
        .and_then(|rest| rest.strip_suffix('/'))
    else {
        return Err(OriginError::Refused(origin));
    };
    let Some(&allowed) = ALLOWED_IDS.iter().find(|allowed| **allowed == id) else {
        return Err(OriginError::Refused(origin));
    };
    for arg in args {
        if !arg
            .to_str()
            .is_some_and(|arg| arg.starts_with("--parent-window="))
        {
            return Err(OriginError::UnknownArgument(
                arg.to_string_lossy().into_owned(),
            ));
        }
    }
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `args` as Chrome passes them, after the program name.
    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    const ORIGIN: &str = "chrome-extension://konofbnhgejkgkonkleipkkpkphbfgee/";

    #[test]
    fn the_extension_origin_is_accepted() {
        assert_eq!(
            parse(args(&[ORIGIN])),
            Ok("konofbnhgejkgkonkleipkkpkphbfgee")
        );
        assert!(is_origin(OsStr::new(ORIGIN)));
        assert!(!is_origin(OsStr::new("daemon")));
    }

    #[test]
    fn windows_parent_window_is_ignored() {
        assert_eq!(
            parse(args(&[ORIGIN, "--parent-window=0"])),
            Ok("konofbnhgejkgkonkleipkkpkphbfgee")
        );
        assert_eq!(
            parse(args(&[ORIGIN, "--other"])),
            Err(OriginError::UnknownArgument("--other".into()))
        );
    }

    #[test]
    fn other_origins_are_refused() {
        for origin in [
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
            "chrome-extension://konofbnhgejkgkonkleipkkpkphbfgee",
            "chrome-extension://konofbnhgejkgkonkleipkkpkphbfgee/x/",
            "moz-extension://konofbnhgejkgkonkleipkkpkphbfgee/",
        ] {
            assert_eq!(
                parse(args(&[origin])),
                Err(OriginError::Refused(origin.into())),
                "{origin}"
            );
        }
        assert_eq!(parse(args(&[])), Err(OriginError::Missing));
    }
}
