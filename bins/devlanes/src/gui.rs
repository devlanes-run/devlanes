//! GUI services (`gui: true`), as this build sets them up. They stay off for
//! users until a real GUI host exists: only a debug build with
//! [`BACKEND_ENV`] set accepts the key, and that variable also picks the
//! backend `devlanes-gui host` runs.

use std::path::PathBuf;

use devlanes_daemon::gui::GuiOptions;

/// Turns GUI services on in a debug build and names the backend, as
/// `devlanes-gui host --backend` takes it: `test-pattern`, the only one there
/// is. Release builds never read it.
pub(crate) const BACKEND_ENV: &str = "DEVLANES_GUI_BACKEND";

/// GUI services for the daemon: on only in a debug build with
/// [`BACKEND_ENV`] set, with [`backend`] and `devlanes-gui` next to the
/// running executable. Off, nothing is looked up.
pub(crate) async fn options() -> GuiOptions {
    let Some(requested) = requested() else {
        return GuiOptions::default();
    };
    let program = tokio::task::spawn_blocking(program)
        .await
        .unwrap_or_else(|_| PathBuf::from(devlanes_update::install::GUI_BINARY));
    GuiOptions {
        enabled: true,
        backend: backend(requested),
        program,
    }
}

/// [`BACKEND_ENV`] in a debug build; `None` in release builds.
fn requested() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(BACKEND_ENV)
        .ok()
        .filter(|backend| !backend.is_empty())
}

/// The backend GUI hosts run on this machine for the `requested` one, or
/// `None` when this OS has none and GUI services run without a host. The
/// test pattern runs everywhere; an OS backend is added here when it exists.
fn backend(requested: String) -> Option<String> {
    (requested == "test-pattern").then_some(requested)
}

/// `devlanes-gui` next to the running `devlanes`, where every install puts
/// it. Blocking: it resolves links.
fn program() -> PathBuf {
    let exe = devlanes_update::running_exe().unwrap_or_else(|_| PathBuf::from("devlanes"));
    exe.with_file_name(devlanes_update::install::GUI_BINARY)
}
