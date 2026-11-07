//! Writes one standalone preview page per fixture, the fake host's scripts and an
//! index into `<target>/mcp-apps-preview/`. Each page takes `?host=`, `?theme=` and
//! `?width=`, and switches them live.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;

use crate::fixtures::{self, NOW, Preview};

const PAGE: &str = include_str!("preview/page.html");
const INDEX: &str = include_str!("preview/index.html");
const HOST: &str = include_str!("preview/host.js");
const HOSTS: &str = include_str!("preview/hosts.js");

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
fn page(preview: &Preview) -> String {
    let fixture = json!({
        "name": preview.name,
        "width": preview.width,
        "displayModes": preview.display_modes,
        "displayMode": preview.display_mode,
        "now": NOW,
        "input": preview.input,
        "result": preview.result,
        "resultDelayMs": preview.result_delay_ms,
        "tools": preview.tools,
        "steps": preview.steps,
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
    PAGE.replace("__TITLE__", &escape(preview.title))
        .replace("__ARGS__", &escape(&format!("Ran devlanes {args}")))
        .replace("__PREVIEW__", &script_json(&fixture))
}

/// Writes every page, the host scripts and the index; returns the pages written.
pub fn write_all(dir: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("create the preview folder");
    fs::write(dir.join("host.js"), HOST).expect("write host.js");
    fs::write(dir.join("hosts.js"), HOSTS).expect("write hosts.js");
    let previews = fixtures::all();
    let mut written = Vec::new();
    let mut links = String::new();
    for preview in &previews {
        let path = dir.join(format!("{}.html", preview.name));
        fs::write(&path, page(preview)).expect("write a preview");
        written.push(path);
        links.push_str(&format!(
            "<li><a href=\"{name}.html\" data-name=\"{name}\">{title}</a></li>",
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
