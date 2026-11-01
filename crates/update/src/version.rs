//! Release versions and their semver ordering, and the version this binary
//! runs.

use std::{cmp::Ordering, sync::OnceLock};

/// Replaces the version [`running`] returns, in debug builds only. A fake
/// release can only be built from the binary under test, which has the same
/// version, and `devlanes update` installs only a newer release; the
/// end-to-end test of `devlanes update` sets this on the install it updates,
/// so that one reports an older version. The release it stages and the
/// binary it then hands over to run without it and report their own.
pub const RUNNING_ENV: &str = "DEVLANES_DEBUG_VERSION";

/// The version this binary reports (`devlanes --version`) and updates from:
/// [`devlanes_version::VERSION`], or in a debug build [`RUNNING_ENV`] when it
/// holds a release version.
pub fn running() -> &'static str {
    if !cfg!(debug_assertions) {
        return devlanes_version::VERSION;
    }
    static RUNNING: OnceLock<String> = OnceLock::new();
    RUNNING.get_or_init(|| {
        std::env::var(RUNNING_ENV)
            .ok()
            .filter(|version| Version::parse_release(version).is_some())
            .unwrap_or_else(|| devlanes_version::VERSION.to_owned())
    })
}

/// A parsed `MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]`, with an optional leading `v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: (u64, u64, u64),
    pre: Vec<String>,
}

impl Version {
    /// `None` for anything that is not a release version.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text
            .split_once('+')
            .map_or(text, |(version, _build)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.').map(number);
        let core = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_owned).collect();
                let valid = ids.iter().all(|id| {
                    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                });
                if !valid {
                    return None;
                }
                ids
            }
            None => Vec::new(),
        };
        Some(Self { core, pre })
    }

    /// `text` when it is a version exactly as a release names it,
    /// `MAJOR.MINOR.PATCH[-PRERELEASE]`: no leading `v`, no build metadata,
    /// no whitespace. Installs and registry answers accept only these, since
    /// the version goes into package specs and download URLs as it is.
    pub fn parse_release(text: &str) -> Option<Self> {
        let plain =
            !text.starts_with('v') && !text.contains('+') && !text.chars().any(char::is_whitespace);
        plain.then(|| Self::parse(text)).flatten()
    }

    /// Whether this is a prerelease, such as a `next` build's `0.9.1-next.3`.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

/// A numeric version component without leading zeros.
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl Ord for Version {
    /// Semver precedence: a prerelease sorts before its release, numeric
    /// identifiers numerically and before alphanumeric ones.
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (left, right) in self.pre.iter().zip(&other.pre) {
                        let order = match (left.parse::<u64>(), right.parse::<u64>()) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => left.cmp(right),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    /// Total order; see [`Ord`].
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Whether `candidate` is a strictly newer release than `current`; false when
/// either does not parse.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// How far a newer release is from the running one. Before 1.0 a minor release
/// may break what worked, so it always asks before installing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bump {
    /// Fixes and additions only.
    Patch,
    /// Another major or minor version, which may break what worked.
    Minor,
}

impl Bump {
    /// The word status and doctor use: `patch` or `minor`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
        }
    }
}

/// The bump from `current` to `latest`; `None` when `latest` is not newer or
/// either does not parse. A prerelease (a `next` build) moving to another
/// `MAJOR.MINOR.PATCH` counts as minor, since the prerelease may already hold
/// changes the other release line lacks.
pub fn bump(current: &str, latest: &str) -> Option<Bump> {
    let (current, latest) = (Version::parse(current)?, Version::parse(latest)?);
    if latest <= current {
        return None;
    }
    let (major, minor, _) = current.core;
    let differs = (major, minor) != (latest.core.0, latest.core.1)
        || (!current.pre.is_empty() && current.core != latest.core);
    Some(if differs { Bump::Minor } else { Bump::Patch })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_precedence() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(is_newer("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(is_newer("1.0.0-rc.10", "1.0.0-rc.9"));
        assert!(is_newer("1.0.0-beta", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.beta", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(!is_newer("0.2.0+build.5", "0.2.0"));
    }

    #[test]
    fn bumps() {
        assert_eq!(bump("0.9.0", "0.9.1"), Some(Bump::Patch));
        assert_eq!(bump("0.9.0", "0.10.0"), Some(Bump::Minor));
        assert_eq!(bump("0.9.3", "1.0.0"), Some(Bump::Minor));
        assert_eq!(bump("0.9.1-next.3", "0.9.1-next.4"), Some(Bump::Patch));
        assert_eq!(bump("0.9.1-next.3", "0.9.1"), Some(Bump::Patch));
        assert_eq!(bump("0.9.1-next.3", "0.9.2"), Some(Bump::Minor));
        assert_eq!(bump("0.9.1", "0.9.1"), None);
        assert_eq!(bump("0.9.1", "0.9.0"), None);
        assert_eq!(bump("local", "0.9.0"), None);
        assert_eq!(Bump::Minor.as_str(), "minor");
    }

    #[test]
    fn rejects_non_versions() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.2.x",
            "1.2.3-",
            "1.2.3-a..b",
            "latest",
        ] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
        assert!(!is_newer("latest", "0.1.0"));
        assert!(!is_newer("0.2.0", "local"));
    }

    #[test]
    fn release_versions_are_exact() {
        for text in ["0.9.1", "0.9.1-next.14", "1.0.0-rc.1"] {
            assert!(Version::parse_release(text).is_some(), "{text}");
        }
        for text in [
            "v0.9.1",
            "0.9.1+x",
            " 0.9.1",
            "0.9.1 ",
            "0.9.1\n",
            "0.9.1-a b",
            "0.9",
            "",
        ] {
            assert_eq!(Version::parse_release(text), None, "{text:?}");
        }
    }
}
