//! The devlanes TUI in the browser for the devlanes.run demo: the real
//! [`devlanes_tui`] app and drawing on its fake daemon ([`devlanes_tui::demo`]),
//! drawn into the page with Ratzilla and driven by keys and a one-second tick
//! from the page's script.
//!
//! The JavaScript API (`Demo`, browser build only) is built with
//! `packaging/scripts/tui-web.sh`, which the release workflow attaches to
//! each release as `devlanes-tui-web.tar.gz`.

pub mod keymap;
pub mod mouse;
pub mod palette;
mod session;
#[cfg(target_family = "wasm")]
mod web;

pub use session::Session;
#[cfg(target_family = "wasm")]
pub use web::Demo;
