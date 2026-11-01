//! The release notes this build carries: every release in
//! `bins/devlanes/CHANGELOG.md` with the first line and highlight of each
//! entry, embedded by `build.rs`, for the release highlights dialog in the
//! TUI.

use std::path::PathBuf;

use devlanes_update::notes::{Entry, Highlight, Level, Release, WhatsNew};

/// One embedded entry: its level, first line and highlight as
/// `(title, body)`.
type Embedded = (Level, &'static str, Option<(&'static str, &'static str)>);

include!(concat!(env!("OUT_DIR"), "/notes.rs"));

/// The embedded releases, newest first, each entry without its body.
pub fn embedded() -> Vec<Release> {
    RELEASES
        .iter()
        .map(|(version, entries)| Release {
            version: (*version).to_owned(),
            entries: entries
                .iter()
                .map(|(level, line, highlight)| Entry {
                    level: *level,
                    line: (*line).to_owned(),
                    body: String::new(),
                    highlight: highlight.map(|(title, body)| Highlight {
                        title: title.to_owned(),
                        body: body.to_owned(),
                    }),
                })
                .collect(),
        })
        .collect()
}

/// Whether this binary is a local build, which never shows the release
/// highlights on its own.
pub fn local_build() -> bool {
    devlanes_version::BUILD_ID == "local"
}

/// The release highlights for a TUI over `home`, which calls it once its
/// terminal is set up: the releases since the version seen before when this
/// is the first TUI after an update ([`devlanes_update::claim_whats_new`]),
/// shown by itself, else this version's own for the Esc menu. `enabled` is
/// `updates.highlights`: off, the version is still recorded as seen.
pub fn for_tui(home: PathBuf, enabled: bool) -> devlanes_tui::WhatsNewClaim {
    std::sync::Arc::new(move || {
        let releases = embedded();
        let running = devlanes_version::VERSION;
        match devlanes_update::claim_whats_new(&home, running, local_build(), enabled, &releases) {
            Some(shown) => (Some(shown), true),
            None => {
                let own = WhatsNew::latest(&releases, running);
                ((!own.is_empty()).then_some(own), false)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changelog_is_embedded_newest_first() {
        let releases = embedded();
        let newest = releases.first().map(|release| release.version.as_str());
        assert_eq!(newest, Some(devlanes_version::VERSION));
        let lines: Vec<&str> = releases
            .iter()
            .flat_map(|release| release.entries.iter().map(|entry| entry.line.as_str()))
            .collect();
        // Commit links are stripped and inline code kept.
        assert!(lines.iter().all(|line| !line.starts_with('[')), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains('`')), "{lines:?}");
        let older = WhatsNew::since(&releases, &releases[2].version, &releases[0].version);
        assert_eq!(older.releases.len(), 2);
    }
}
