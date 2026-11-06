//! Checks on the card document, its snapshots of fixture data and the preview pages.

mod fixtures;
mod preview;

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

/// Most hosts load the resource on every card; this keeps it from growing unnoticed.
const BUDGET_BYTES: usize = 80 * 1024;

/// The only URLs the card may contain: XML namespaces Preact sets on SVG
/// elements, which are names and never fetched.
const ALLOWED_URLS: &[&str] = &["http://www.w3.org/2000/svg", "http://www.w3.org/1999/xlink"];

#[test]
fn the_card_fits_its_size_budget() {
    let size = devlanes_mcp_ui::html().len();
    assert!(
        size <= BUDGET_BYTES,
        "the card is {size} bytes, over its budget of {BUDGET_BYTES}"
    );
}

/// The error pages and the card share the website's tokens but cannot share one SCSS
/// partial (each crate compiles its own stylesheet), so this keeps the copies equal.
#[test]
fn the_card_and_the_error_pages_share_tokens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let card = fs::read_to_string(root.join("ui/styles/_tokens.scss")).expect("read the tokens");
    let pages = fs::read_to_string(root.join("../pages/styles/page.scss")).expect("read pages");
    let card_value = |name: &str| {
        card.lines()
            .find_map(|line| line.strip_prefix(&format!("${name}: ")))
            .map(|value| value.trim_end_matches(';').to_owned())
            .unwrap_or_else(|| panic!("_tokens.scss has no ${name}"))
    };
    let pages_has = |line: &str| pages.lines().any(|candidate| candidate.trim() == line);
    for (card_name, pages_line) in [
        ("surface-light", "--surface: {};"),
        ("surface-dark", "--surface: {};"),
        ("paint", "$paint: {};"),
        ("terminal", "$terminal: {};"),
        ("mono", "$mono: {};"),
        ("prose", "$prose: {};"),
    ] {
        let line = pages_line.replace("{}", &card_value(card_name));
        assert!(pages_has(&line), "crates/pages/styles/page.scss lacks `{line}`");
    }
}

#[test]
fn the_vendored_module_is_the_pinned_release() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/vendor");
    let module = fs::read(dir.join("standalone.module.js")).expect("read the vendored module");
    let readme = fs::read_to_string(dir.join("README.md")).expect("read ui/vendor/README.md");
    let pinned = readme
        .lines()
        .find_map(|line| line.strip_prefix("SHA-256: "))
        .map(|hash| hash.trim().trim_matches('`'))
        .expect("ui/vendor/README.md records the SHA-256");
    let actual: String = Sha256::digest(&module)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(actual, pinned, "ui/vendor/standalone.module.js changed");
    // The card ships the checked file: its code up to the export statement is inline.
    let text = String::from_utf8(module).expect("the module is UTF-8");
    let body = &text[..text.rfind("export{").expect("the module exports")];
    assert!(devlanes_mcp_ui::html().contains(body.trim_end()));
}

#[test]
fn the_card_makes_no_requests() {
    let html = devlanes_mcp_ui::html();
    let mut rest = html;
    while let Some(start) = rest.find("http") {
        let url: String = rest[start..]
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | '`' | ' ' | ')' | '<'))
            .collect();
        if url.starts_with("http://") || url.starts_with("https://") {
            assert!(ALLOWED_URLS.contains(&url.as_str()), "the card names {url}");
        }
        rest = &rest[start + 4..];
    }
    assert!(
        html.contains("default-src &#39;none&#39;"),
        "the card's own policy blocks every request"
    );
}

#[test]
fn fixture_results_match_their_snapshots() {
    for preview in fixtures::all() {
        insta::assert_json_snapshot!(preview.name, preview.structured());
    }
}

/// Writes the preview pages; open `index.html` in that folder through a local
/// server to look at every view (see the crate README).
#[test]
fn previews_are_written() {
    let dir = preview::dir();
    let written = preview::write_all(&dir);
    for path in &written {
        let page = fs::read_to_string(path).expect("read a preview");
        assert!(page.contains("devlanes"), "{} is empty", path.display());
    }
    println!("{} previews in {}", written.len(), dir.display());
}
