//! Text for the `devlanes gui` commands. The binary builds their JSON (the
//! services and windows it found, the file a screenshot went to, the input
//! it sent) and JSON output prints it unchanged.

use serde_json::Value;

use super::{Ctx, clean, int, list, table, text};
use crate::GuiOutput;

/// Lines for the result of a `devlanes gui` command.
pub(crate) fn result(output: GuiOutput, data: &Value, cx: &Ctx<'_>) -> Vec<String> {
    match output {
        GuiOutput::Ls => ls(data, cx),
        GuiOutput::Screenshot => screenshot(data, cx),
        GuiOutput::Input => input(data),
    }
}

/// `lane:unit`, with `on box NAME` for a box's lane.
fn target(data: &Value) -> String {
    let mut target = format!("{}:{}", text(data, "lane"), text(data, "unit"));
    let host = text(data, "box");
    if !host.is_empty() {
        target.push_str(&format!(" on box {host}"));
    }
    target
}

/// `window 2 "Editor"`, from `{id, title}`.
fn window(window: &Value) -> String {
    let id = int(window, "id").unwrap_or_default();
    match text(window, "title") {
        title if title.is_empty() => format!("window {id}"),
        title => format!("window {id} {title:?}"),
    }
}

/// `gui ls`: a line per service with its host's state and address, and a
/// row per window under it.
fn ls(data: &Value, cx: &Ctx<'_>) -> Vec<String> {
    let lane = text(data, "lane");
    let items = list(data, "items");
    if items.is_empty() {
        return vec![format!("no GUI services in {lane}")];
    }
    let mut lines = Vec::new();
    for item in items {
        // The host's state: `running`, `stopped` with its unit, `exited`
        // while the unit runs on, `none` on a machine without a GUI host, or
        // `unreachable` when its port did not answer.
        let (meaning, word) = match text(item, "host").as_str() {
            "running" => ("running", "running".to_owned()),
            "exited" => ("failed", "host exited".to_owned()),
            "unreachable" => ("failed", "unreachable".to_owned()),
            "none" => ("idle", "no host".to_owned()),
            other => ("stopped", other.to_owned()),
        };
        let mut line = format!(
            "{} {}:{}",
            cx.state_text(meaning, &word),
            lane,
            text(item, "unit")
        );
        let address = text(item, "address");
        if !address.is_empty() {
            line.push_str(&format!(" {address}"));
        }
        if item.get("real_desktop").and_then(Value::as_bool) == Some(true) {
            line.push_str(" on this machine's desktop");
        }
        let host_box = text(data, "box");
        if !host_box.is_empty() {
            line.push_str(&format!(" on box {host_box}"));
        }
        let error = text(item, "error");
        if !error.is_empty() {
            line.push_str(&format!(": {error}"));
        } else if item.get("unmirrored").is_some_and(|value| !value.is_null()) {
            line.push_str(&format!(
                ": not mirrored here, {}",
                super::words::gui_unmirrored(&item["unmirrored"])
            ));
        }
        lines.push(line);
        let rows: Vec<Vec<String>> = list(item, "windows")
            .iter()
            .map(|window| {
                let mut row = vec![
                    format!("window {}", int(window, "id").unwrap_or_default()),
                    format!(
                        "{}x{}",
                        int(window, "width").unwrap_or_default(),
                        int(window, "height").unwrap_or_default()
                    ),
                    text(window, "kind"),
                ];
                match text(window, "lock") {
                    lock if lock == "unlocked" || lock.is_empty() => row.push(String::new()),
                    lock => row.push(lock),
                }
                row.push(format!("{:?}", text(window, "title")));
                row
            })
            .collect();
        if cx.agent() {
            lines.extend(rows.iter().map(|row| {
                row.iter()
                    .filter(|cell| !cell.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            }));
        } else {
            lines.extend(table(&rows, "  "));
        }
    }
    lines
}

/// `gui screenshot`: the file, a link to it where the terminal shows links.
fn screenshot(data: &Value, cx: &Ctx<'_>) -> Vec<String> {
    let path = text(data, "path");
    let shown = match cx.runtime.terminal.links.filter(|_| cx.human()) {
        Some(links) => links.file(&path, std::path::Path::new(&path), None, None),
        None => path.clone(),
    };
    vec![format!(
        "wrote {shown} ({}x{}, {} of {})",
        int(data, "width").unwrap_or_default(),
        int(data, "height").unwrap_or_default(),
        window(&data["window"]),
        target(data)
    )]
}

/// The input commands: what the app got, and what became of the window.
/// Nothing here says the app drew its result, which a slow app may not have
/// done yet.
fn input(data: &Value) -> Vec<String> {
    let place = format!("{} of {}", window(&data["window"]), target(data));
    let point = |key: &str| {
        let at = &data[key];
        format!(
            "{},{}",
            int(at, "x").unwrap_or_default(),
            int(at, "y").unwrap_or_default()
        )
    };
    let line = match text(data, "action").as_str() {
        "click" => {
            let verb = if data.get("double").and_then(Value::as_bool) == Some(true) {
                "double-clicked"
            } else {
                "clicked"
            };
            format!(
                "{verb} {} at {} in {place}",
                text(data, "button"),
                point("at")
            )
        }
        "type" => {
            let count = int(data, "characters").unwrap_or_default();
            let noun = if count == 1 {
                "character"
            } else {
                "characters"
            };
            format!("typed {count} {noun} into {place}")
        }
        "key" => {
            let keys: Vec<String> = list(data, "keys")
                .iter()
                .filter_map(Value::as_str)
                .map(clean)
                .collect();
            format!("pressed {} in {place}", keys.join(", "))
        }
        "scroll" => format!(
            "scrolled {} lines at {} in {place}",
            point("delta"),
            point("at")
        ),
        "resize" => {
            let size = |key: &str| {
                let size = &data[key];
                format!(
                    "{}x{}",
                    int(size, "width").unwrap_or_default(),
                    int(size, "height").unwrap_or_default()
                )
            };
            let (asked, took) = (size("requested"), size("size"));
            if data["size"].is_null() {
                format!("asked {place} for {asked}")
            } else if asked == took {
                format!("resized {place} to {took}")
            } else {
                format!("asked {place} for {asked}; it is {took}")
            }
        }
        other => format!("sent {} to {place}", clean(other)),
    };
    // What became of the window once the app had the input.
    let after = match text(data, "window_after").as_str() {
        "" | "open" => String::new(),
        "closed" => "; the window closed after it".into(),
        "outside" => {
            let size = &data["window_size"];
            format!(
                "; the window is {}x{} now, so the point may have been outside it and the input dropped",
                int(size, "width").unwrap_or_default(),
                int(size, "height").unwrap_or_default()
            )
        }
        lock => format!("; the window is {lock} now, so it takes no input until that ends"),
    };
    vec![format!("{line}{after}")]
}
