//! The MCP Apps card of `devlanes mcp`: one self-contained HTML document that chat
//! hosts render next to a tool result. It shows a lane, the lanes board, a failure
//! with its logs, the clean planner or a pairing code, picked from the result's
//! `structuredContent.view`, and reads fresh data through the server's app-only
//! tools. `build.rs` assembles it from `ui/`; it makes no requests of its own.

/// The resource URI tools name in `_meta.ui.resourceUri`.
pub const URI: &str = "ui://devlanes/lane.html";

/// The MIME type of the resource, as MCP Apps requires.
pub const MIME_TYPE: &str = "text/html;profile=mcp-app";

/// The document `build.rs` writes.
const HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/lane.html"));

/// The whole card: HTML with its stylesheet and script inline, for `resources/read`.
pub fn html() -> &'static str {
    HTML
}
