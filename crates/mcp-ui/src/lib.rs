//! The MCP Apps card of `devlanes mcp`: one self-contained HTML document that chat
//! hosts render next to a tool result. It shows a lane, the lanes board, a failure
//! with its logs, the clean planner or a pairing code, picked from the result's
//! `structuredContent.view`, and reads fresh data through the server's app-only
//! tools. `pnpm build` in `ui/` writes it to `ui/dist/lane.html`, which is
//! committed, so building devlanes needs no Node; it makes no requests of its own.

use std::sync::LazyLock;

/// The resource URI tools name in `_meta.ui.resourceUri`.
pub const URI: &str = "ui://devlanes/lane.html";

/// The MIME type of the resource, as MCP Apps requires.
pub const MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Where the committed card holds the devlanes version, in a meta tag outside the
/// hashed script and stylesheet, so a release changes neither the file nor its
/// Content-Security-Policy.
pub const VERSION_PLACEHOLDER: &str = "__DEVLANES_VERSION__";

/// The committed build of `ui/`.
const BUILT: &str = include_str!("../ui/dist/lane.html");

/// The card with this release's version filled in.
static HTML: LazyLock<String> =
    LazyLock::new(|| BUILT.replace(VERSION_PLACEHOLDER, devlanes_version::VERSION));

/// The whole card: HTML with its stylesheet and script inline, for `resources/read`.
pub fn html() -> &'static str {
    &HTML
}
