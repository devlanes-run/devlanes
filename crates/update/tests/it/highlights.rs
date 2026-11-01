//! The highlight comments in `.sampo/changesets/` read as the release
//! highlights dialog will read them once Sampo copies them into the changelog.

use std::path::Path;

use devlanes_update::notes::{BODY_MAX, TITLE_MAX, split_highlight};

/// Every pending change file with something that looks like a highlight
/// comment must hold a closed `<!-- highlight` comment with a title and a
/// body within the limits, so a mistake fails here instead of being cut,
/// dropped or swallowing the rest of a release at release time.
#[test]
fn pending_change_files_hold_valid_highlights() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sampo/changesets");
    let Ok(files) = std::fs::read_dir(&dir) else {
        return;
    };
    for file in files.map(|file| file.expect("entry").path()) {
        if file.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("change file");
        let name = file.file_name().expect("name").to_string_lossy();
        // Past the front matter, whose closing line is the second `---`.
        let body = text.splitn(3, "---").nth(2).unwrap_or_default();
        // Looser than the parser, so a typo such as `<!--highlights` is caught.
        let looks_like_one = body
            .lines()
            .any(|line| line.trim_start().starts_with("<!--") && line.contains("highlight"));
        if !looks_like_one {
            continue;
        }
        let (rest, highlight) = split_highlight(body);
        assert_ne!(
            rest, body,
            "{name}: the highlight comment must be `<!-- highlight`, then `title:` and `body:` lines, then `-->`"
        );
        let highlight = highlight
            .unwrap_or_else(|| panic!("{name}: the highlight needs a `title:` and a `body:`"));
        assert!(
            highlight.title.chars().count() <= TITLE_MAX,
            "{name}: the title is over {TITLE_MAX} characters"
        );
        assert!(
            highlight.body.chars().count() <= BODY_MAX,
            "{name}: the body is over {BODY_MAX} characters"
        );
    }
}
