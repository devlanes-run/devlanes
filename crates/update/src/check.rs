//! Release checks: which registry tells the latest version for an install
//! method and channel, reading it with `curl`, and the release notes of a
//! version. The daemon schedules the checks and caches their answers.

use std::{process::Stdio, time::Duration};

use serde_json::Value;

use crate::{InstallMethod, notes, version::Version};

/// Upper bound on a registry answer. PyPI's JSON lists every file of every
/// release, so it is the largest; it stays far below this for years.
const MAX_LATEST_BYTES: usize = 8 * 1024 * 1024;

/// Upper bound on `CHANGELOG.md`, which grows by a few KiB per release.
const MAX_CHANGELOG_BYTES: usize = 8 * 1024 * 1024;

/// A line of releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Tagged releases, published to every registry.
    Stable,
    /// Prereleases built from the main branch (`0.9.1-next.14`), published only
    /// as release archives on GitHub.
    Next,
}

impl Channel {
    /// The word config, status and the cache use: `stable` or `next`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Next => "next",
        }
    }

    /// The channel a build of `version` follows when the config names none:
    /// `next` for a version with `-next.`.
    pub fn of_version(version: &str) -> Self {
        if version.contains("-next.") {
            Self::Next
        } else {
            Self::Stable
        }
    }
}

/// Where the latest version of an install comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The release manifest on GitHub, for `install.sh`, `install.ps1`,
    /// standalone binaries and cargo.
    GitHub(Channel),
    /// The npm registry, for npm, pnpm, Bun and Yarn.
    Npm,
    /// PyPI, for uv, uvx, pipx and pip.
    PyPi,
    /// The Homebrew tap's formula.
    Homebrew,
}

impl Source {
    /// The source for `method` following `channel`, or why it has none:
    /// `local` for a project dependency, which its manifest pins, and `source`
    /// for a build from a checkout. Only `install.sh`, `install.ps1` and
    /// standalone binaries follow `next`; the registries carry stable releases
    /// alone, so every other method reads its stable source.
    pub fn for_method(method: &InstallMethod, channel: Channel) -> Result<Self, &'static str> {
        Ok(match method {
            InstallMethod::Script(_) => Self::GitHub(channel),
            InstallMethod::CargoInstall | InstallMethod::CargoBinstall => {
                Self::GitHub(Channel::Stable)
            }
            InstallMethod::Npm | InstallMethod::Pnpm | InstallMethod::Bun | InstallMethod::Yarn => {
                Self::Npm
            }
            InstallMethod::Uv
            | InstallMethod::Uvx
            | InstallMethod::Pipx
            | InstallMethod::PipxRun
            | InstallMethod::Pip(_)
            | InstallMethod::UvPip(_) => Self::PyPi,
            InstallMethod::Homebrew => Self::Homebrew,
            InstallMethod::Local { .. } => return Err("local"),
            InstallMethod::Source => return Err("source"),
        })
    }

    /// The word status and the cache use: `github`, `npm`, `pypi` or `homebrew`.
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub(_) => "github",
            Self::Npm => "npm",
            Self::PyPi => "pypi",
            Self::Homebrew => "homebrew",
        }
    }

    /// The channel this source serves.
    pub fn channel(self) -> Channel {
        match self {
            Self::GitHub(channel) => channel,
            Self::Npm | Self::PyPi | Self::Homebrew => Channel::Stable,
        }
    }
}

/// The addresses a check reads. Tests point them at a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Urls {
    /// The latest stable release's manifest (`{"version": ...}`).
    pub github_stable: String,
    /// The newest `next` prerelease's manifest.
    pub github_next: String,
    /// The npm registry's `latest` document (`{"version": ...}`).
    pub npm: String,
    /// PyPI's JSON for the project (`{"info": {"version": ...}}`).
    pub pypi: String,
    /// The tap's formula, whose download URLs hold the version.
    pub homebrew: String,
    /// `CHANGELOG.md` at a release tag, with `{version}` where the version goes.
    pub changelog: String,
}

impl Default for Urls {
    /// The real registries.
    fn default() -> Self {
        let raw = "https://raw.githubusercontent.com/devlanes-run";
        Self {
            github_stable: devlanes_version::LATEST_RELEASE_MANIFEST_URL.into(),
            github_next: devlanes_version::NEXT_RELEASE_MANIFEST_URL.into(),
            npm: "https://registry.npmjs.org/devlanes/latest".into(),
            pypi: "https://pypi.org/pypi/devlanes/json".into(),
            homebrew: format!("{raw}/homebrew-tap/main/Formula/devlanes.rb"),
            changelog: format!("{raw}/devlanes/v{{version}}/bins/devlanes/CHANGELOG.md"),
        }
    }
}

impl Urls {
    /// Where `source` tells the latest version.
    pub fn latest(&self, source: Source) -> &str {
        match source {
            Source::GitHub(Channel::Stable) => &self.github_stable,
            Source::GitHub(Channel::Next) => &self.github_next,
            Source::Npm => &self.npm,
            Source::PyPi => &self.pypi,
            Source::Homebrew => &self.homebrew,
        }
    }

    /// `CHANGELOG.md` as released in `version`.
    pub fn changelog(&self, version: &str) -> String {
        self.changelog.replace("{version}", version)
    }
}

/// Why a check found no version.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// The request took longer than its limit.
    #[error("timed out")]
    Timeout,
    /// `curl` could not be started.
    #[error("curl: {0}")]
    Curl(#[source] std::io::Error),
    /// `curl` failed: a network error, an HTTP error status or a refused redirect.
    #[error("{0}")]
    Request(String),
    /// The answer passed the size limit.
    #[error("answer too large")]
    TooLarge,
    /// The answer holds no release version.
    #[error("{0}")]
    Invalid(String),
}

/// Reads `url` with curl, which uses the system's trust store and proxy
/// settings and follows GitHub's redirects to its asset host. Only HTTPS is
/// allowed, redirects included, except for a plain HTTP URL a test serves.
pub async fn fetch(url: &str, timeout: Duration, max_bytes: usize) -> Result<Vec<u8>, CheckError> {
    let protocols = if url.starts_with("https://") {
        "=https"
    } else {
        "=http,https"
    };
    let seconds = timeout.as_secs().max(1).to_string();
    let output = tokio::time::timeout(
        timeout + Duration::from_secs(1),
        tokio::process::Command::new("curl")
            .args(["-fsSL", "--max-time", &seconds, "--proto", protocols])
            .args(["--proto-redir", protocols, "--max-filesize"])
            .arg(max_bytes.to_string())
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| CheckError::Timeout)?
    .map_err(CheckError::Curl)?;
    if !output.status.success() {
        return Err(CheckError::Request(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    // `--max-filesize` trusts the announced length; a chunked answer has none.
    if output.stdout.len() > max_bytes {
        return Err(CheckError::TooLarge);
    }
    Ok(output.stdout)
}

/// The latest release version `source` names, read from `urls`.
pub async fn latest(source: Source, urls: &Urls, timeout: Duration) -> Result<String, CheckError> {
    let body = fetch(urls.latest(source), timeout, MAX_LATEST_BYTES).await?;
    parse_latest(source, &body)
}

/// The release version in a `source`'s answer, as semver without a leading `v`.
pub fn parse_latest(source: Source, body: &[u8]) -> Result<String, CheckError> {
    let invalid =
        |what: &str| CheckError::Invalid(format!("{} answer has no {what}", source.name()));
    let version = match source {
        Source::GitHub(_) | Source::Npm => {
            json_string(body, &["version"]).ok_or_else(|| invalid("version"))?
        }
        Source::PyPi => {
            let text = json_string(body, &["info", "version"]).ok_or_else(|| invalid("version"))?;
            pep440_to_semver(&text).ok_or_else(|| {
                CheckError::Invalid(format!("PyPI version {text:?} is not a release version"))
            })?
        }
        Source::Homebrew => formula_version(&String::from_utf8_lossy(body))
            .ok_or_else(|| invalid("download URL with a version"))?,
    };
    let version = version.trim().trim_start_matches('v');
    Version::parse(version)
        .map(|_| version.to_owned())
        .ok_or_else(|| CheckError::Invalid(format!("{version:?} is not a release version")))
}

/// The releases in `CHANGELOG.md` as released in `version`.
pub async fn release_notes(
    urls: &Urls,
    version: &str,
    timeout: Duration,
) -> Result<Vec<notes::Release>, CheckError> {
    let body = fetch(&urls.changelog(version), timeout, MAX_CHANGELOG_BYTES).await?;
    Ok(notes::parse(&String::from_utf8_lossy(&body)))
}

/// The string at `path` in a JSON document.
fn json_string(body: &[u8], path: &[&str]) -> Option<String> {
    let root: Value = serde_json::from_slice(body).ok()?;
    let mut value = &root;
    for key in path {
        value = value.get(key)?;
    }
    value.as_str().map(str::to_owned)
}

/// The semver form of a PyPI version: `1.2.0a1` is `1.2.0-alpha.1`, `b` is
/// beta and `rc` is rc. A zero number has no suffix (`1.2.0rc0` is `1.2.0-rc`),
/// since the wheels write devlanes' first prerelease of a series that way.
pub fn pep440_to_semver(text: &str) -> Option<String> {
    let text = text.trim();
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (release, pre) = text.split_at(split);
    if pre.is_empty() {
        return Some(release.to_owned());
    }
    let digits = pre.find(|c: char| c.is_ascii_digit()).unwrap_or(pre.len());
    let (label, number) = pre.split_at(digits);
    let label = match label {
        "a" => "alpha",
        "b" => "beta",
        "rc" => "rc",
        _ => return None,
    };
    let number: u64 = number.parse().ok()?;
    Some(match number {
        0 => format!("{release}-{label}"),
        number => format!("{release}-{label}.{number}"),
    })
}

/// The version in the first `/download/v<version>/` of a formula's `url` lines.
pub fn formula_version(formula: &str) -> Option<String> {
    formula
        .lines()
        .filter(|line| line.trim_start().starts_with("url "))
        .find_map(|line| {
            let (_, rest) = line.split_once("/download/v")?;
            let (version, _) = rest.split_once('/')?;
            Some(version.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn sources_by_method_and_channel() {
        let script = InstallMethod::Script(PathBuf::from("/opt/acme/bin"));
        assert_eq!(
            Source::for_method(&script, Channel::Next),
            Ok(Source::GitHub(Channel::Next))
        );
        assert_eq!(
            Source::for_method(&InstallMethod::CargoInstall, Channel::Next),
            Ok(Source::GitHub(Channel::Stable))
        );
        assert_eq!(
            Source::for_method(&InstallMethod::Pnpm, Channel::Next),
            Ok(Source::Npm)
        );
        assert_eq!(
            Source::for_method(&InstallMethod::UvPip(PathBuf::new()), Channel::Stable),
            Ok(Source::PyPi)
        );
        assert_eq!(
            Source::for_method(&InstallMethod::Homebrew, Channel::Stable),
            Ok(Source::Homebrew)
        );
        let local = InstallMethod::Local {
            manifest: PathBuf::from("package.json"),
        };
        assert_eq!(Source::for_method(&local, Channel::Stable), Err("local"));
        assert_eq!(
            Source::for_method(&InstallMethod::Source, Channel::Stable),
            Err("source")
        );
        assert_eq!(Source::Npm.channel(), Channel::Stable);
        assert_eq!(Channel::of_version("0.9.1-next.14"), Channel::Next);
        assert_eq!(Channel::of_version("0.9.1-rc.1"), Channel::Stable);
    }

    #[test]
    fn reads_each_registry() {
        let github = Source::GitHub(Channel::Stable);
        assert_eq!(
            parse_latest(github, br#"{"version":"v0.9.1","assets":[]}"#).ok(),
            Some("0.9.1".into())
        );
        assert_eq!(
            parse_latest(Source::Npm, br#"{"name":"devlanes","version":"0.9.1"}"#).ok(),
            Some("0.9.1".into())
        );
        assert_eq!(
            parse_latest(Source::PyPi, br#"{"info":{"version":"0.10.0rc2"}}"#).ok(),
            Some("0.10.0-rc.2".into())
        );
        let formula = "class Devlanes < Formula\n  version \"0.9.1\"\n  on_macos do\n    url \"https://github.com/devlanes-run/devlanes/releases/download/v0.9.1/devlanes-0.9.1-aarch64-apple-darwin.tar.gz\"\n";
        assert_eq!(
            parse_latest(Source::Homebrew, formula.as_bytes()).ok(),
            Some("0.9.1".into())
        );
        assert!(parse_latest(Source::Npm, br#"{"version":"latest"}"#).is_err());
        assert!(parse_latest(Source::PyPi, b"not json").is_err());
        assert!(parse_latest(Source::Homebrew, b"class Devlanes < Formula\n").is_err());
    }

    #[test]
    fn pep440_maps_back_to_semver() {
        assert_eq!(pep440_to_semver("1.2.0").as_deref(), Some("1.2.0"));
        assert_eq!(
            pep440_to_semver("1.2.0a1").as_deref(),
            Some("1.2.0-alpha.1")
        );
        assert_eq!(pep440_to_semver("1.2.0b3").as_deref(), Some("1.2.0-beta.3"));
        assert_eq!(pep440_to_semver("1.2.0rc0").as_deref(), Some("1.2.0-rc"));
        assert_eq!(
            pep440_to_semver("1.2.0rc12").as_deref(),
            Some("1.2.0-rc.12")
        );
        assert_eq!(pep440_to_semver("1.2.0.dev1"), None);
        assert_eq!(pep440_to_semver("1.2.0post1"), None);
    }

    #[test]
    fn changelog_url_names_the_tag() {
        assert_eq!(
            Urls::default().changelog("0.9.1"),
            "https://raw.githubusercontent.com/devlanes-run/devlanes/v0.9.1/bins/devlanes/CHANGELOG.md"
        );
    }
}
