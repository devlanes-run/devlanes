//! Whether a viewer could open windows: one rule for the viewer,
//! `devlanes gui open` and the daemon's auto-open.

use std::ffi::OsString;

/// The variables a Unix desktop other than macOS is reached through, as
/// winit reads them.
pub const DISPLAY_VARIABLES: [&str; 3] = ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"];

/// Whether an environment that `read` looks variables up in has a display:
/// always on macOS and Windows, which open windows in the session; on other
/// Unix systems when one of [`DISPLAY_VARIABLES`] is set and not empty.
pub fn has_display(read: impl Fn(&str) -> Option<OsString>) -> bool {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return true;
    }
    DISPLAY_VARIABLES
        .iter()
        .any(|name| read(name).is_some_and(|value| !value.is_empty()))
}

/// [`has_display`] for this process's environment.
pub fn here() -> bool {
    has_display(|name| std::env::var_os(name))
}
