//! Release notes from devlanes' `CHANGELOG.md`, as the release pull request
//! writes it: a `## X.Y.Z — date` section per release, `### Minor changes` and
//! `### Patch changes` headings, and entries `- [sha](url) First line.` whose
//! further lines are indented by two spaces. An entry may carry a
//! `<!-- highlight ... -->` comment that Sampo copies from the change file
//! (see [`split_highlight`]). Uses only the standard library (and the version
//! constants), so a build script can parse the changelog it ships with.

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
    /// The lines after it without their two-space indent, or empty. A
    /// `<!-- highlight ... -->` comment is not part of it.
    pub body: String,
    /// What the author wrote in the entry's highlight comment, if any.
    pub highlight: Option<Highlight>,
}

/// The text a change file offers for the release highlights dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// A few words, at most [`TITLE_MAX`] characters.
    pub title: String,
    /// One or two sentences, at most [`BODY_MAX`] characters.
    pub body: String,
}

/// The longest highlight title, in characters.
pub const TITLE_MAX: usize = 60;

/// The longest highlight body, in characters.
pub const BODY_MAX: usize = 300;

/// The most non-breaking highlights the dialog lists; the rest are counted.
pub const HIGHLIGHTS_MAX: usize = 5;

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
                highlight: None,
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
    for entry in releases
        .iter_mut()
        .flat_map(|release| release.entries.iter_mut())
    {
        let (body, highlight) = split_highlight(&entry.body);
        entry.body = body;
        entry.highlight = highlight.map(Highlight::clamped);
    }
    releases
}

impl Highlight {
    /// The highlight with its title and body cut to [`TITLE_MAX`] and
    /// [`BODY_MAX`] characters, so a long one in an old changelog cannot
    /// fill the dialog.
    fn clamped(self) -> Self {
        Self {
            title: clamp(&self.title, TITLE_MAX),
            body: clamp(&self.body, BODY_MAX),
        }
    }
}

/// `text` cut to `max` characters, ending in `…` when it was cut.
fn clamp(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Splits the `<!-- highlight ... -->` comment out of `text`, a change
/// file's body or a changelog entry's, which holds `title:` and `body:`
/// lines (a line that starts with neither continues the one before):
///
/// ```text
/// <!-- highlight
/// title: Update in place
/// body: Run `devlanes update` to install the latest release.
/// -->
/// ```
///
/// Returns the text without the comment and the highlight, with its fields
/// as written. A comment missing its title or body is removed and gives
/// `None`; a text without the comment comes back unchanged. The change file
/// test and [`parse`] share this, so Sampo's copy of the comment into the
/// changelog reads the same as the file.
pub fn split_highlight(text: &str) -> (String, Option<Highlight>) {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|line| opens_highlight(line)) else {
        return (text.to_owned(), None);
    };
    // An unclosed comment runs to the end of the text.
    let end = (start..lines.len())
        .find(|&index| lines[index].contains("-->"))
        .unwrap_or(lines.len() - 1);
    let inner = lines[start..=end].join("\n");
    let inner = inner
        .trim_start()
        .trim_start_matches("<!--")
        .trim_start()
        .trim_start_matches("highlight");
    let inner = inner.split("-->").next().unwrap_or_default();

    let (mut title, mut body) = (String::new(), String::new());
    // Whether the last key seen was `title` (`true`) or `body`.
    let mut in_title: Option<bool> = None;
    for line in inner.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(rest) = line.strip_prefix("title:") {
            title = rest.trim().to_owned();
            in_title = Some(true);
        } else if let Some(rest) = line.strip_prefix("body:") {
            body = rest.trim().to_owned();
            in_title = Some(false);
        } else if let Some(is_title) = in_title {
            let field = if is_title { &mut title } else { &mut body };
            field.push(' ');
            field.push_str(line);
        }
    }

    let rest = lines[..start]
        .iter()
        .chain(&lines[end + 1..])
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let highlight = (!title.is_empty() && !body.is_empty()).then_some(Highlight { title, body });
    (rest.trim().to_owned(), highlight)
}

/// Whether `line` starts a `<!-- highlight` comment.
fn opens_highlight(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("<!--")
        .is_some_and(|rest| rest.trim_start().starts_with("highlight"))
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

/// The releases the release highlights dialog draws from: one release, or
/// every release since the version seen before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhatsNew {
    /// The version seen before; `None` when it shows one release's notes.
    pub since: Option<String>,
    /// Newest first, the minor entries of each before its patch entries.
    pub releases: Vec<Release>,
}

impl WhatsNew {
    /// The releases newer than `after` up to `up_to` (see [`between`]).
    pub fn since(releases: &[Release], after: &str, up_to: &str) -> Self {
        Self {
            since: Some(after.to_owned()),
            releases: between(releases, after, up_to)
                .into_iter()
                .map(minor_first)
                .collect(),
        }
    }

    /// The notes of `version` alone; empty when `releases` has no section
    /// for it.
    pub fn of(releases: &[Release], version: &str) -> Self {
        let wanted = Version::parse(version);
        Self {
            since: None,
            releases: releases
                .iter()
                .filter(|release| wanted.is_some() && Version::parse(&release.version) == wanted)
                .take(1)
                .map(minor_first)
                .collect(),
        }
    }

    /// The notes of the newest release in `releases` not newer than
    /// `version`: its own section, or for a `next` build the release it
    /// builds on. Empty when there is none.
    pub fn latest(releases: &[Release], version: &str) -> Self {
        match between(releases, "0.0.0", version).first() {
            Some(newest) => Self::of(releases, &newest.version),
            None => Self {
                since: None,
                releases: Vec::new(),
            },
        }
    }

    /// Whether there is nothing to show: no breaking entry and no highlight.
    /// Ordinary entries stay on the release page.
    pub fn is_empty(&self) -> bool {
        self.releases.iter().all(|release| {
            release
                .entries
                .iter()
                .all(|entry| entry.level == Level::Patch && entry.highlight.is_none())
        })
    }

    /// What the dialog lists: every breaking entry, then the highlights of
    /// the other entries, newest release first, at most [`HIGHLIGHTS_MAX`]
    /// with the rest counted in [`Summary::more`]. A breaking entry with a
    /// highlight is listed once, in its highlight's words.
    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for entry in self.releases.iter().flat_map(|release| &release.entries) {
            match (entry.level, &entry.highlight) {
                (Level::Minor, highlight) => summary.breaking.push(Item {
                    breaking: true,
                    title: highlight.as_ref().map(|found| found.title.clone()),
                    text: highlight
                        .as_ref()
                        .map_or_else(|| entry.line.clone(), |found| found.body.clone()),
                }),
                (Level::Patch, Some(found)) if summary.highlights.len() < HIGHLIGHTS_MAX => {
                    summary.highlights.push(Item {
                        breaking: false,
                        title: Some(found.title.clone()),
                        text: found.body.clone(),
                    });
                }
                (Level::Patch, Some(_)) => summary.more += 1,
                (Level::Patch, None) => {}
            }
        }
        summary
    }

    /// `Release highlights in 0.9.1` for one release, `Release highlights
    /// since 0.8.0` for several.
    pub fn title(&self) -> String {
        match (self.releases.as_slice(), &self.since) {
            ([only], _) => format!("Release highlights in {}", only.version),
            ([_, _, ..], Some(since)) => format!("Release highlights since {since}"),
            ([newest, ..], None) => format!("Release highlights in {}", newest.version),
            ([], _) => "Release highlights".to_owned(),
        }
    }

    /// Where the full notes are: the release page of the only release shown,
    /// the list of releases when several are.
    pub fn url(&self) -> String {
        match self.releases.as_slice() {
            [only] => release_url(&only.version),
            _ => format!("{}/releases", devlanes_version::REPOSITORY),
        }
    }
}

/// One line of the release highlights dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Whether it may break what worked.
    pub breaking: bool,
    /// The highlight's title; `None` for a breaking entry without one.
    pub title: Option<String>,
    /// The highlight's body, or the entry's first line.
    pub text: String,
}

/// What [`WhatsNew::summary`] picks for the dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Every breaking entry, newest release first.
    pub breaking: Vec<Item>,
    /// The first [`HIGHLIGHTS_MAX`] highlights of non-breaking entries.
    pub highlights: Vec<Item>,
    /// Highlights past that limit, left to the release page.
    pub more: usize,
}

/// The GitHub release page of `version`.
pub fn release_url(version: &str) -> String {
    format!("{}/releases/tag/v{version}", devlanes_version::REPOSITORY)
}

/// `release` with its minor entries first, each group in file order.
fn minor_first(release: &Release) -> Release {
    let mut release = release.clone();
    release
        .entries
        .sort_by_key(|entry| entry.level != Level::Minor);
    release
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
        // The comment is not part of the body, and a continued line joins.
        assert_eq!(
            first.entries[0].highlight,
            Some(Highlight {
                title: "Choose the box for a lane".into(),
                body: "Run `devlanes remote use` or press `m` in the TUI to move a lane's hostnames and ports to another box.".into(),
            })
        );
        assert_eq!(first.entries[1].highlight, None);
        assert_eq!(
            first.entries[2]
                .highlight
                .as_ref()
                .map(|found| found.title.as_str()),
            Some("Look up a lane's domain")
        );

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
    fn whats_new_puts_minor_entries_first_and_names_its_range() {
        let mut releases = parse(FIXTURE);
        // A patch entry above a minor one still shows below it.
        releases[0].entries.rotate_right(1);
        let levels = |shown: &WhatsNew| -> Vec<(String, Level)> {
            shown
                .releases
                .iter()
                .flat_map(|release| {
                    release
                        .entries
                        .iter()
                        .map(|entry| (release.version.clone(), entry.level))
                })
                .collect()
        };

        let since = WhatsNew::since(&releases, "0.8.0", "0.9.0");
        assert_eq!(since.title(), "Release highlights since 0.8.0");
        assert_eq!(
            levels(&since),
            [
                ("0.9.0".into(), Level::Minor),
                ("0.9.0".into(), Level::Minor),
                ("0.9.0".into(), Level::Patch),
                ("0.8.1".into(), Level::Patch),
            ]
        );
        assert_eq!(
            WhatsNew::since(&releases, "0.8.1", "0.9.0").title(),
            "Release highlights in 0.9.0"
        );

        let one = WhatsNew::of(&releases, "0.8.1");
        assert_eq!(one.title(), "Release highlights in 0.8.1");
        assert_eq!(one.releases.len(), 1);
        assert!(WhatsNew::of(&releases, "0.9.1").is_empty());
        // A `next` build shows the release it builds on.
        assert_eq!(
            WhatsNew::latest(&releases, "0.9.1-next.4").title(),
            "Release highlights in 0.9.0"
        );
        assert!(WhatsNew::latest(&releases, "0.1.0").is_empty());
    }

    #[test]
    fn a_release_with_only_ordinary_entries_has_nothing_to_show() {
        let releases = parse(FIXTURE);
        // 0.8.1 has one patch entry and no highlight.
        assert!(WhatsNew::of(&releases, "0.8.1").is_empty());
        assert!(!WhatsNew::of(&releases, "0.9.0").is_empty());
        // A breaking entry without a highlight is enough on its own.
        let mut breaking_only = WhatsNew::of(&releases, "0.8.0");
        breaking_only.releases[0]
            .entries
            .retain(|entry| entry.level == Level::Minor);
        assert!(!breaking_only.is_empty());
        assert!(breaking_only.summary().highlights.is_empty());
    }

    #[test]
    fn the_summary_lists_breaking_entries_then_highlights() {
        let releases = parse(FIXTURE);
        let summary = WhatsNew::since(&releases, "0.7.1", "0.9.0").summary();
        let breaking: Vec<(Option<&str>, &str)> = summary
            .breaking
            .iter()
            .map(|item| (item.title.as_deref(), item.text.as_str()))
            .collect();
        // A breaking entry with a highlight is listed once, in its words.
        assert_eq!(breaking[0].0, Some("Choose the box for a lane"));
        assert!(breaking[0].1.starts_with("Run `devlanes remote use`"));
        assert_eq!(
            breaking[1],
            (
                None,
                "Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost`."
            )
        );
        assert_eq!(breaking.len(), 3);
        assert!(summary.breaking.iter().all(|item| item.breaking));
        let highlights: Vec<&str> = summary
            .highlights
            .iter()
            .filter_map(|item| item.title.as_deref())
            .collect();
        assert_eq!(
            highlights,
            ["Look up a lane's domain", "Reload a running lane"]
        );
        assert_eq!(summary.more, 0);
    }

    #[test]
    fn highlights_past_the_limit_are_counted_not_listed() {
        let entry = |number: usize| Entry {
            level: Level::Patch,
            line: format!("Added thing {number}."),
            body: String::new(),
            highlight: Some(Highlight {
                title: format!("Thing {number}"),
                body: "Does it.".into(),
            }),
        };
        let shown = WhatsNew {
            since: None,
            releases: vec![
                Release {
                    version: "0.9.0".into(),
                    entries: (0..4).map(entry).collect(),
                },
                Release {
                    version: "0.8.0".into(),
                    entries: (4..8).map(entry).collect(),
                },
            ],
        };
        let summary = shown.summary();
        assert_eq!(summary.highlights.len(), HIGHLIGHTS_MAX);
        // The newest releases fill the list first.
        assert_eq!(summary.highlights[4].title.as_deref(), Some("Thing 4"));
        assert_eq!(summary.more, 3);
    }

    #[test]
    fn the_release_page_follows_the_releases_shown() {
        let releases = parse(FIXTURE);
        assert_eq!(
            WhatsNew::of(&releases, "0.9.0").url(),
            format!("{}/releases/tag/v0.9.0", devlanes_version::REPOSITORY)
        );
        assert_eq!(
            WhatsNew::since(&releases, "0.7.1", "0.9.0").url(),
            format!("{}/releases", devlanes_version::REPOSITORY)
        );
    }

    #[test]
    fn split_highlight_reads_keys_and_removes_the_comment() {
        let (rest, found) = split_highlight(
            "Run it.\n\n<!-- highlight\ntitle: Run it\nbody: Does the thing\n  across two lines.\n-->\n\nMore.",
        );
        assert_eq!(rest, "Run it.\n\n\nMore.");
        assert_eq!(
            found,
            Some(Highlight {
                title: "Run it".into(),
                body: "Does the thing across two lines.".into(),
            })
        );
        // No comment: the text comes back as it was.
        assert_eq!(split_highlight("  plain \n"), ("  plain \n".into(), None));
        // A comment missing its body is removed and yields nothing.
        let (rest, found) = split_highlight("Line.\n<!-- highlight\ntitle: Only a title\n-->");
        assert_eq!((rest.as_str(), found), ("Line.", None));
        // Other comments stay.
        assert_eq!(
            split_highlight("<!-- note -->").0,
            "<!-- note -->".to_owned()
        );
    }

    #[test]
    fn long_highlights_are_cut_when_the_changelog_is_parsed() {
        let long = "x".repeat(400);
        let changelog = format!(
            "## 1.0.0\n\n### Patch changes\n\n- Added a thing.\n  \n  <!-- highlight\n  title: {long}\n  body: {long}\n  -->\n"
        );
        let found = parse(&changelog)[0].entries[0]
            .highlight
            .clone()
            .expect("highlight");
        assert_eq!(found.title.chars().count(), TITLE_MAX);
        assert_eq!(found.body.chars().count(), BODY_MAX);
        assert!(found.title.ends_with('…'));
        // Under the limit nothing changes, and the raw split keeps it whole.
        assert_eq!(clamp("short", TITLE_MAX), "short");
        let (_, raw) = split_highlight(&format!("<!-- highlight\ntitle: {long}\nbody: b\n-->"));
        assert_eq!(raw.expect("highlight").title.len(), 400);
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
