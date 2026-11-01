//! Release notes from devlanes' `CHANGELOG.md`, as the release pull request
//! writes it: a `## X.Y.Z — date` section per release, `### Minor changes` and
//! `### Patch changes` headings, and entries `- [sha](url) First line.` whose
//! further lines are indented by two spaces. Uses only the standard library,
//! so a build script can parse the changelog it ships with.

use crate::version::Version;

/// One release's section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version in the heading, without a leading `v`.
    pub version: String,
    /// Its entries in file order: minor changes, then patch changes.
    pub entries: Vec<Entry>,
}

/// One change in a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The heading the entry sits under.
    pub level: Level,
    /// The first line, without the commit link.
    pub line: String,
    /// The lines after it without their two-space indent, or empty.
    pub body: String,
}

/// The heading an entry sits under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// `### Minor changes`: may break what worked, before 1.0.
    Minor,
    /// `### Patch changes`.
    Patch,
}

impl Level {
    /// The word status uses: `minor` or `patch`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minor => "minor",
            Self::Patch => "patch",
        }
    }
}

/// The releases in `markdown`, in file order (newest first in devlanes'
/// changelog). Sections whose heading holds no version, and entries under
/// headings other than minor and patch changes, are left out.
pub fn parse(markdown: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    // Whether the current section counts, and the level of its heading.
    let mut in_release = false;
    let mut level: Option<Level> = None;
    // Blank lines seen inside an entry, kept only when more body follows.
    let mut blanks = 0usize;
    let mut in_entry = false;
    for raw in markdown.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(heading) = line.strip_prefix("## ") {
            in_entry = false;
            level = None;
            in_release = match heading_version(heading) {
                Some(version) => {
                    releases.push(Release {
                        version,
                        entries: Vec::new(),
                    });
                    true
                }
                None => false,
            };
            continue;
        }
        if let Some(heading) = line.strip_prefix("### ") {
            in_entry = false;
            level = match heading.trim().to_ascii_lowercase().as_str() {
                "minor changes" => Some(Level::Minor),
                "patch changes" => Some(Level::Patch),
                _ => None,
            };
            continue;
        }
        if line.starts_with('#') {
            in_entry = false;
            in_release = false;
            continue;
        }
        let (true, Some(level), Some(release)) = (in_release, level, releases.last_mut()) else {
            continue;
        };
        if let Some(text) = line.strip_prefix("- ") {
            release.entries.push(Entry {
                level,
                line: without_commit_link(text).trim().to_owned(),
                body: String::new(),
            });
            in_entry = true;
            blanks = 0;
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some(entry) = release.entries.last_mut() else {
            continue;
        };
        if line.trim().is_empty() {
            blanks += 1;
        } else if let Some(text) = line.strip_prefix("  ") {
            if !entry.body.is_empty() {
                entry.body.push('\n');
                for _ in 0..blanks {
                    entry.body.push('\n');
                }
            }
            blanks = 0;
            entry.body.push_str(text);
        } else {
            // Unindented text ends the entry.
            in_entry = false;
        }
    }
    releases
}

/// The releases newer than `after` and not newer than `up_to`, newest first.
/// Releases whose version does not parse are left out.
pub fn between<'a>(releases: &'a [Release], after: &str, up_to: &str) -> Vec<&'a Release> {
    let (Some(after), Some(up_to)) = (Version::parse(after), Version::parse(up_to)) else {
        return Vec::new();
    };
    let mut found: Vec<(Version, &Release)> = releases
        .iter()
        .filter_map(|release| Some((Version::parse(&release.version)?, release)))
        .filter(|(version, _)| *version > after && *version <= up_to)
        .collect();
    found.sort_by(|left, right| right.0.cmp(&left.0));
    found.into_iter().map(|(_, release)| release).collect()
}

/// The version in a `X.Y.Z — date` heading (an em dash, or `-` in hand-written
/// files); `None` when the first word is not a version.
fn heading_version(heading: &str) -> Option<String> {
    let word = heading.split_whitespace().next()?;
    let word = word.strip_prefix('v').unwrap_or(word);
    Version::parse(word).map(|_| word.to_owned())
}

/// `text` without a leading `[sha](url) ` commit link.
fn without_commit_link(text: &str) -> &str {
    let Some(rest) = text.strip_prefix('[') else {
        return text;
    };
    let Some((_, after)) = rest.split_once("](") else {
        return text;
    };
    match after.split_once(')') {
        Some((_, tail)) => tail.trim_start(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/CHANGELOG.md");

    #[test]
    fn parses_sections_levels_links_and_bodies() {
        let releases = parse(FIXTURE);
        let versions: Vec<&str> = releases.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.9.0", "0.8.1", "0.8.0", "0.7.1"]);

        let first = &releases[0];
        assert_eq!(first.entries.len(), 3);
        assert_eq!(first.entries[0].level, Level::Minor);
        assert_eq!(
            first.entries[0].line,
            "Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane."
        );
        assert_eq!(
            first.entries[0].body,
            "A switch moves the lane's hostnames and ports to the other box at once.\n\nSee the guide."
        );
        assert_eq!(first.entries[2].level, Level::Patch);
        assert_eq!(first.entries[2].body, "");

        let fenced = &releases[2].entries[2];
        assert_eq!(
            fenced.line,
            "Added a top-level `extends` key that builds a project file on others."
        );
        assert_eq!(
            fenced.body,
            "```yaml\n# devlanes.prod.yaml\nextends: devlanes.yaml\nprofiles:\n  default: { include: [\"*\"] }\n```"
        );
        // The hyphen heading and an entry without a commit link.
        assert_eq!(
            releases[3].entries[0].line,
            "Fixed CI checkouts getting lane 1."
        );
        assert_eq!(releases[3].entries[0].level, Level::Patch);
    }

    #[test]
    fn between_is_exclusive_below_and_inclusive_above() {
        let releases = parse(FIXTURE);
        let versions = |after, up_to| -> Vec<String> {
            between(&releases, after, up_to)
                .into_iter()
                .map(|release| release.version.clone())
                .collect()
        };
        assert_eq!(versions("0.7.1", "0.9.0"), ["0.9.0", "0.8.1", "0.8.0"]);
        assert_eq!(versions("0.8.0", "0.8.1"), ["0.8.1"]);
        assert_eq!(versions("0.9.0", "0.9.0"), Vec::<String>::new());
        assert_eq!(versions("junk", "0.9.0"), Vec::<String>::new());
    }

    #[test]
    fn skips_other_headings_and_text() {
        let releases = parse(
            "# devlanes\n\nIntro.\n\n## Unreleased\n\n- not a release\n\n## 1.2.3\n\n### Notes\n\n- skipped\n\n### Patch changes\n\n- kept\nloose text\n  not body\n",
        );
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].entries.len(), 1);
        assert_eq!(releases[0].entries[0].line, "kept");
        assert_eq!(releases[0].entries[0].body, "");
    }
}
