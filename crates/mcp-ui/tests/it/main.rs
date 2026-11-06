//! Checks on the card document, its snapshots of fixture data and the preview pages.

mod fixtures;
mod preview;

use std::{fs, path::Path};

use base64::Engine;
use sha2::{Digest, Sha256};

/// Most hosts load the resource on every card; this keeps it from growing unnoticed.
const BUDGET_BYTES: usize = 80 * 1024;

/// The only URLs the card may contain: XML namespaces Preact sets on elements,
/// which are names the card fetches nothing from, and the one origin its policy
/// lets it load host fonts from.
const ALLOWED_URLS: &[&str] = &[
    "http://www.w3.org/2000/svg",
    "http://www.w3.org/1999/xhtml",
    "http://www.w3.org/1998/Math/MathML",
    FONT_ORIGIN,
];

/// Where Claude serves the fonts its `styles.css.fonts` names; the card's
/// `font-src` and the server's `resourceDomains` list it and nothing else.
const FONT_ORIGIN: &str = "https://assets.claude.ai";

/// The loopback addresses of mirrored box ports, which the open button hands to
/// the host; the minifier renames the port variable inside `${...}`.
const ALLOWED_URL_TEMPLATES: &[&str] = &["http://127.0.0.1:${", "http://[::1]:${"];

/// The committed build of `ui/`, as `pnpm build` wrote it.
fn committed() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist/lane.html");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The text of every `<tag>...</tag>` element in `html`.
fn bodies<'a>(html: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let body_start = after.find('>').expect("the tag closes") + 1;
        let body_end = after.find(&close).expect("the element ends");
        found.push(&after[body_start..body_end]);
        rest = &after[body_end + close.len()..];
    }
    found
}

/// The `'sha256-...'` source of a CSP hash.
fn hash_source(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

#[test]
fn the_card_fits_its_size_budget() {
    let size = devlanes_mcp_ui::html().len();
    assert!(
        size <= BUDGET_BYTES,
        "the card is {size} bytes, over its budget of {BUDGET_BYTES}"
    );
}

/// The card's fallback palette (the devlanes look, for hosts that send no style
/// variables) is the website's, as the error pages have it. The two cannot share
/// one SCSS partial (grass compiles the pages, Sass in `ui/` the card), so this
/// keeps the copies equal.
#[test]
fn the_card_fallback_palette_matches_the_error_pages() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let card =
        fs::read_to_string(root.join("ui/src/styles/_tokens.scss")).expect("read the tokens");
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
        ("ink-light", "--ink: {};"),
        ("muted-light", "--muted: {};"),
        ("line-light", "--line: {};"),
        ("info-light", "--info: {};"),
        ("warning-light", "--warning: {};"),
        ("error-light", "--error: {};"),
        ("terminal", "$terminal: {};"),
        ("terminal-ink", "$terminal-ink: {};"),
        ("paint", "$paint: {};"),
        ("mono", "$mono: {};"),
        ("prose", "$prose: {};"),
    ] {
        let line = pages_line.replace("{}", &card_value(card_name));
        assert!(
            pages_has(&line),
            "crates/pages/styles/page.scss lacks `{line}`"
        );
    }
}

/// The binary serves the committed file with only the version filled in. A file
/// a merge left stale or broken fails here; CI's `pnpm build` check catches one
/// that no longer matches `ui/src`.
#[test]
fn the_card_is_the_committed_build() {
    let committed = committed();
    assert_eq!(
        committed
            .matches(devlanes_mcp_ui::VERSION_PLACEHOLDER)
            .count(),
        1,
        "ui/dist/lane.html holds the version placeholder once"
    );
    assert_eq!(
        devlanes_mcp_ui::html(),
        committed.replace(
            devlanes_mcp_ui::VERSION_PLACEHOLDER,
            devlanes_version::VERSION
        )
    );
    for marker in ["<<<<<<<", ">>>>>>>"] {
        assert!(
            !committed.contains(marker),
            "ui/dist/lane.html has merge conflict markers; run `pnpm build` in crates/mcp-ui/ui"
        );
    }
}

/// The policy allows the card's own inline script and stylesheet by hash and
/// nothing else, so a hand edit of the built file stops it from running.
#[test]
fn the_card_policy_allows_only_its_own_code() {
    let html = devlanes_mcp_ui::html();
    let csp = html
        .split("http-equiv=\"Content-Security-Policy\" content=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the card has a Content-Security-Policy meta tag");
    let directive = |name: &str| {
        csp.split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix(name))
            .map(str::trim)
            .unwrap_or_else(|| panic!("the policy has no {name}"))
    };
    assert_eq!(directive("default-src"), "'none'");
    assert_eq!(directive("font-src"), FONT_ORIGIN);
    for (name, tag) in [("script-src", "script"), ("style-src", "style")] {
        let expected: Vec<String> = bodies(html, tag).into_iter().map(hash_source).collect();
        assert!(!expected.is_empty(), "the card has no inline {tag}");
        assert_eq!(
            directive(name),
            expected.join(" "),
            "{name} lists exactly the hashes of the inline {tag} elements"
        );
    }
}

#[test]
fn the_card_makes_no_requests() {
    let html = devlanes_mcp_ui::html();
    let mut rest = html;
    while let Some(start) = rest.find("http") {
        let url: String = rest[start..]
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | '`' | ' ' | ')' | '<' | ';'))
            .collect();
        if url.starts_with("http://") || url.starts_with("https://") {
            assert!(
                ALLOWED_URLS.contains(&url.as_str())
                    || ALLOWED_URL_TEMPLATES
                        .iter()
                        .any(|prefix| url.starts_with(prefix)),
                "the card names {url}"
            );
        }
        rest = &rest[start + 4..];
    }
    assert!(
        !html.contains(" src=") && !html.contains(" href="),
        "the card loads no file"
    );
}

#[test]
fn fixture_results_match_their_snapshots() {
    for preview in fixtures::all() {
        insta::assert_json_snapshot!(preview.name, preview.structured());
        if let Some(boxes) = preview.remote_ls() {
            insta::assert_json_snapshot!(format!("{}-remote-ls", preview.name), boxes);
        }
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
