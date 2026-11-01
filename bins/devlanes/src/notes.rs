//! The release notes this build carries: every release in
//! `bins/devlanes/CHANGELOG.md` with the first line of each entry, embedded
//! by `build.rs`, for "What's new" in the TUI and `devlanes update`.

use std::path::PathBuf;

use devlanes_update::notes::{Entry, Level, Release, WhatsNew};

include!(concat!(env!("OUT_DIR"), "/notes.rs"));

/// The embedded releases, newest first, each entry without its body.
pub fn embedded() -> Vec<Release> {
    RELEASES
        .iter()
        .map(|(version, entries)| Release {
            version: (*version).to_owned(),
            entries: entries
                .iter()
                .map(|(level, line)| Entry {
                    level: *level,
                    line: (*line).to_owned(),
                    body: String::new(),
                })
                .collect(),
        })
        .collect()
}

/// Whether this binary is a local build, which never shows "What's new" on
/// its own.
pub fn local_build() -> bool {
    devlanes_version::BUILD_ID == "local"
}

/// "What's new" for a TUI over `home`, which calls it once its terminal is
/// set up: the releases since the version seen before when this is the first
/// TUI after an update ([`devlanes_update::claim_whats_new`]), shown by
/// itself, else this version's own notes for the Esc menu.
pub fn for_tui(home: PathBuf) -> devlanes_tui::WhatsNewClaim {
    std::sync::Arc::new(move || {
        let releases = embedded();
        let running = devlanes_version::VERSION;
        match devlanes_update::claim_whats_new(&home, running, local_build(), &releases) {
            Some(shown) => (Some(shown), true),
            None => {
                let own = WhatsNew::latest(&releases, running);
                ((!own.is_empty()).then_some(own), false)
            }
        }
    })
}

/// `shown` as terminal lines: the title, then each release's version and
/// its entries, minor ones marked `breaking`.
pub fn render(shown: &WhatsNew) -> String {
    let mut out = format!("{}\n", shown.title());
    for release in &shown.releases {
        out.push_str(&format!("\n  {}\n", release.version));
        for entry in &release.entries {
            let marker = match entry.level {
                Level::Minor => "breaking  ",
                Level::Patch => "",
            };
            out.push_str(&format!("    {marker}{}\n", entry.line));
        }
    }
    out
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

    #[test]
    fn rendered_notes_mark_breaking_entries() {
        let shown = WhatsNew {
            since: Some("0.8.0".into()),
            releases: vec![Release {
                version: "0.9.0".into(),
                entries: vec![
                    Entry {
                        level: Level::Minor,
                        line: "Removed `--acme`.".into(),
                        body: String::new(),
                    },
                    Entry {
                        level: Level::Patch,
                        line: "Fixed a crash.".into(),
                        body: String::new(),
                    },
                ],
            }],
        };
        assert_eq!(
            render(&shown),
            "What's new in 0.9.0\n\n  0.9.0\n    breaking  Removed `--acme`.\n    Fixed a crash.\n"
        );
    }
}
