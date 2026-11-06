//! Writes one standalone preview page per fixture and theme, plus an index,
//! into `<target>/mcp-apps-preview/`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::fixtures::{self, NOW, Preview};

const PAGE: &str = include_str!("preview/page.html");
const INDEX: &str = include_str!("preview/index.html");
const HOST: &str = include_str!("preview/host.js");

/// The preview folder: `mcp-apps-preview` in the target directory, which
/// `CARGO_TARGET_TMPDIR` (`<target>/tmp`) sits in.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("CARGO_TARGET_TMPDIR is inside the target directory")
        .join("mcp-apps-preview")
}

/// JSON that is safe inside an inline `<script>`.
fn script_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .expect("fixtures serialize")
        .replace('<', "\\u003c")
}

/// Text that is safe in HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One preview page around the real card.
fn page(preview: &Preview, theme: &str) -> String {
    let fixture = json!({
        "name": preview.name,
        "theme": theme,
        "width": preview.width,
        "displayModes": preview.display_modes,
        "now": NOW,
        "input": preview.input,
        "result": preview.result,
        "resultDelayMs": preview.result_delay_ms,
        "tools": preview.tools,
        "card": devlanes_mcp_ui::html(),
    });
    let args = preview.input["args"]
        .as_array()
        .map(|args| {
            args.iter()
                .filter_map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    PAGE.replace("__THEME__", theme)
        .replace("__TITLE__", &escape(preview.title))
        .replace(
            "__ARGS__",
            &escape(&format!("the agent called devlanes with: {args}")),
        )
        .replace("__HOST__", HOST)
        .replace("__PREVIEW__", &script_json(&fixture))
}

/// Writes every page and the index; returns the files written.
pub fn write_all(dir: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("create the preview folder");
    let previews = fixtures::all();
    let mut written = Vec::new();
    let mut links = String::new();
    for preview in &previews {
        for theme in ["light", "dark"] {
            let path = dir.join(format!("{}-{theme}.html", preview.name));
            fs::write(&path, page(preview, theme)).expect("write a preview");
            written.push(path);
        }
        links.push_str(&format!(
            "<li><a href=\"{name}-light.html\" data-name=\"{name}\">{title}</a></li>",
            name = preview.name,
            title = escape(preview.title)
        ));
    }
    let index = INDEX
        .replace("__LINKS__", &links)
        .replace("__FIRST__", previews[0].name);
    let path = dir.join("index.html");
    fs::write(&path, index).expect("write the preview index");
    written.push(path);
    written
}
