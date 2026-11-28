//! Script installs against a fake release directory served as `file://`:
//! staging checks the checksum, the archive's entries and the version the
//! staged binary reports, and applying renames the binaries into place.
//! Needs `curl` and `tar`; on Windows, `fake-tool` stands in for
//! `devlanes.exe` (build it with `cargo build -p devlanes-testing`).

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use devlanes_update::{
    check::Urls,
    install::{self, BINARIES, GUI_BINARY, InstallError, Lock, Release, Staged, TARGET},
};

/// The archive format of this OS's releases.
#[cfg(unix)]
const SUFFIX: &str = ".tar.gz";
#[cfg(windows)]
const SUFFIX: &str = ".zip";

/// A temporary release site, devlanes home and install directory.
struct Site {
    _root: tempfile::TempDir,
    /// The directory served as the releases' base URL.
    base: PathBuf,
    home: PathBuf,
    /// The install directory, holding an old `devlanes`.
    dir: PathBuf,
}

impl Site {
    /// Empty directories and an installed `devlanes` that says `old`.
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let base = root.path().join("site");
        let home = root.path().join("home");
        let dir = root.path().join("bin");
        for path in [&base, &home, &dir] {
            std::fs::create_dir_all(path).expect("dir");
        }
        for name in BINARIES {
            std::fs::write(dir.join(name), "old").expect("old binary");
        }
        Self {
            _root: root,
            base,
            home,
            dir,
        }
    }

    /// The release addresses under this site.
    fn urls(&self) -> Urls {
        Urls::under(&file_url(&self.base))
    }

    /// Publishes `version` at `releases/download/v<version>/`: an archive
    /// whose `devlanes` reports `reports`, with `extra` added next to the
    /// release files, and a `SHA256SUMS` with `sha` (the real one when
    /// `None`). Returns the staged binary's contents.
    fn publish(
        &self,
        version: &str,
        reports: &str,
        extra: Option<&str>,
        sha: Option<&str>,
    ) -> Vec<u8> {
        let top = format!("devlanes-{version}-{TARGET}");
        let work = self.base.join("work").join(version);
        let package = work.join(&top);
        std::fs::create_dir_all(&package).expect("package");
        std::fs::write(package.join("LICENSE"), "Apache-2.0").expect("license");
        fake_binary(&package, &work, "devlanes", &format!("devlanes {reports}"));
        for name in BINARIES.iter().skip(1) {
            std::fs::write(package.join(name), "helper").expect("helper");
        }
        if let Some(extra) = extra {
            std::fs::write(package.join(extra), "extra").expect("extra");
        }
        let folder = self
            .base
            .join("releases/download")
            .join(format!("v{version}"));
        std::fs::create_dir_all(&folder).expect("folder");
        let archive_name = format!("{top}{SUFFIX}");
        let archive = folder.join(&archive_name);
        let status = Command::new(tar())
            .args(TAR_CREATE)
            .arg(&archive)
            .arg("-C")
            .arg(&work)
            .arg(&top)
            .status()
            .expect("tar");
        assert!(status.success(), "tar failed");
        let actual = install::sha256_file(&archive).expect("sha");
        std::fs::write(
            folder.join("SHA256SUMS"),
            format!("{}  {archive_name}\n", sha.unwrap_or(&actual)),
        )
        .expect("sums");
        std::fs::read(package.join(BINARIES[0])).expect("binary")
    }

    /// Adds the GUI archive to the release of `version` that [`Site::publish`]
    /// wrote, with a `devlanes-gui` whose `version` prints `devlanes-gui
    /// <reports> build=release` and `extra` next to it, and lists it in
    /// `SHA256SUMS` after the main archive. Returns the binary's contents.
    fn publish_gui(&self, version: &str, reports: &str, extra: Option<&str>) -> Vec<u8> {
        let top = format!("devlanes-{version}-{TARGET}");
        let work = self.base.join("work").join(format!("{version}-gui"));
        let package = work.join(&top);
        std::fs::create_dir_all(&package).expect("package");
        std::fs::write(package.join("LICENSE"), "Apache-2.0").expect("license");
        let line = format!("devlanes-gui {reports} build=release");
        fake_binary(&package, &work, "devlanes-gui", &line);
        if let Some(extra) = extra {
            std::fs::write(package.join(extra), "extra").expect("extra");
        }
        let folder = self
            .base
            .join("releases/download")
            .join(format!("v{version}"));
        let name = install::gui_archive_name(version, TARGET);
        let archive = folder.join(&name);
        let status = Command::new(tar())
            .args(TAR_CREATE)
            .arg(&archive)
            .arg("-C")
            .arg(&work)
            .arg(&top)
            .status()
            .expect("tar");
        assert!(status.success(), "tar failed");
        let sums = folder.join("SHA256SUMS");
        let mut text = std::fs::read_to_string(&sums).expect("sums");
        let sha = install::sha256_file(&archive).expect("sha");
        text.push_str(&format!("{sha}  {name}\n"));
        std::fs::write(&sums, text).expect("sums");
        std::fs::read(package.join(GUI_BINARY)).expect("binary")
    }

    /// Where staging unpacks `version`.
    fn staged_files(&self, version: &str) -> PathBuf {
        self.home
            .join("update")
            .join(version)
            .join(format!("devlanes-{version}-{TARGET}"))
    }

    /// Stages `version` under the lock.
    async fn stage(&self, version: &str) -> Result<Staged, InstallError> {
        // fake-tool reads its behaviour next to itself; staging keeps files
        // already in the version's folder.
        #[cfg(windows)]
        {
            let files = self.staged_files(version);
            std::fs::create_dir_all(&files).expect("staged files");
            let work = self.base.join("work");
            for (dir, name) in [
                (version.to_owned(), "devlanes.exe.json"),
                (format!("{version}-gui"), "devlanes-gui.exe.json"),
            ] {
                let behaviour = work.join(dir).join(name);
                if behaviour.is_file() {
                    std::fs::copy(behaviour, files.join(name)).expect("behaviour");
                }
            }
        }
        let home = self.home.clone();
        let _lock = tokio::task::spawn_blocking(move || Lock::acquire(&home, Duration::ZERO))
            .await
            .expect("join")?;
        install::stage(
            &self.home,
            &self.urls(),
            &Release::Version(version.into()),
            &self.dir,
            Duration::from_secs(60),
        )
        .await
    }
}

/// `file:///...` for a local directory.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    format!("file:///{}", path.trim_start_matches('/'))
}

/// `tar` flags that create this OS's archive format.
#[cfg(unix)]
const TAR_CREATE: &[&str] = &["-czf"];
#[cfg(windows)]
const TAR_CREATE: &[&str] = &["-a", "-cf"];

/// The `tar` that reads and writes this OS's archives.
fn tar() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join(r"System32\tar.exe");
    #[cfg(unix)]
    PathBuf::from("tar")
}

/// A program `name` in `package` that prints `line` whatever its arguments:
/// a shell script, or on Windows `fake-tool` with its behaviour written to
/// `work`, outside the archive.
#[cfg(unix)]
fn fake_binary(package: &Path, _work: &Path, name: &str, line: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = package.join(name);
    std::fs::write(&path, format!("#!/bin/sh\necho '{line}'\n")).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
}

#[cfg(windows)]
fn fake_binary(package: &Path, work: &Path, name: &str, line: &str) {
    use devlanes_testing::fake_tool::{Behaviour, install};
    let program =
        install(work, name, &Behaviour::new().stdout(format!("{line}\n"))).expect("fake-tool");
    std::fs::rename(&program, package.join(format!("{name}.exe"))).expect("move");
}

/// Applies `staged` under the lock.
async fn apply(site: &Site, staged: Staged) {
    let home = site.home.clone();
    let applied = tokio::task::spawn_blocking(move || {
        let _lock = Lock::acquire(&home, Duration::ZERO)?;
        install::apply(&home, &staged)
    })
    .await
    .expect("join")
    .expect("applied");
    assert!(applied.in_use.is_empty(), "{applied:?}");
}

/// A release from before devlanes-gui, with no GUI archive, installs as it
/// always did; the next release brings devlanes-gui from its own archive.
#[tokio::test]
async fn a_staged_release_is_applied_over_the_install() {
    let site = Site::new();
    let binary = site.publish("0.9.1", "0.9.1", None, None);
    let staged = site.stage("0.9.1").await.expect("staged");
    assert_eq!(staged.version, "0.9.1");
    assert_eq!(staged.target, site.dir);
    assert_eq!(staged.files, site.staged_files("0.9.1"));
    assert!(!staged.gui);
    assert_eq!(Staged::load(&site.home), Some(staged.clone()));
    assert_eq!(
        std::fs::read(staged.files.join(BINARIES[0])).expect("staged"),
        binary
    );
    // The downloaded archive is not kept.
    assert!(
        !site
            .home
            .join("update/0.9.1")
            .join(format!("devlanes-0.9.1-{TARGET}{SUFFIX}"))
            .exists()
    );
    apply(&site, staged).await;
    assert_eq!(
        std::fs::read(site.dir.join(BINARIES[0])).expect("installed"),
        binary
    );
    assert!(!site.dir.join(GUI_BINARY).exists());

    // A newer staged release replaces the older one.
    site.publish("0.9.2", "0.9.2", None, None);
    site.publish_gui("0.9.2", "0.9.2", None);
    site.stage("0.9.2").await.expect("staged");
    site.publish("0.9.3", "0.9.3", None, None);
    let gui_binary = site.publish_gui("0.9.3", "0.9.3", None);
    let newer = site.stage("0.9.3").await.expect("staged");
    assert!(!site.home.join("update/0.9.2").exists());
    assert!(newer.gui);
    assert_eq!(Staged::load(&site.home), Some(newer.clone()));
    let newer_binary = std::fs::read(newer.files.join(BINARIES[0])).expect("binary");
    assert_ne!(newer_binary, b"old");
    assert!(
        !site
            .home
            .join("update/0.9.3")
            .join(install::gui_archive_name("0.9.3", TARGET))
            .exists()
    );

    apply(&site, newer).await;
    assert_eq!(
        std::fs::read(site.dir.join(BINARIES[0])).expect("installed"),
        newer_binary
    );
    assert_eq!(
        std::fs::read(site.dir.join(GUI_BINARY)).expect("installed"),
        gui_binary
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in BINARIES.iter().chain([&GUI_BINARY]) {
            let mode = std::fs::metadata(site.dir.join(name))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "{name}");
        }
        assert_eq!(
            std::fs::read(site.dir.join("devlanes-hosts")).expect("helper"),
            b"helper"
        );
    }
    assert_eq!(Staged::load(&site.home), None);
    assert!(!site.home.join("update/0.9.3").exists());
    let leftovers: Vec<_> = std::fs::read_dir(&site.dir)
        .expect("dir")
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn staging_refuses_a_release_that_does_not_check_out() {
    let site = Site::new();
    site.publish("0.9.1", "0.9.1", None, Some(&"0".repeat(64)));
    let error = site.stage("0.9.1").await.expect_err("bad checksum");
    assert!(
        error.to_string().starts_with("checksum mismatch for"),
        "{error}"
    );

    site.publish("0.9.2", "0.9.2", Some("README"), None);
    let error = site.stage("0.9.2").await.expect_err("extra entry");
    assert!(
        error.to_string().starts_with("unexpected entries in"),
        "{error}"
    );

    site.publish("0.9.3", "0.9.0", None, None);
    let error = site.stage("0.9.3").await.expect_err("wrong version");
    assert_eq!(
        error.to_string(),
        "the 0.9.3 archive's devlanes reports 0.9.0"
    );

    // The GUI archive is checked like the main one, and fails the whole stage.
    site.publish("0.9.4", "0.9.4", None, None);
    site.publish_gui("0.9.4", "0.9.4", Some("README"));
    let error = site.stage("0.9.4").await.expect_err("extra GUI entry");
    assert!(
        error.to_string().starts_with(&format!(
            "unexpected entries in {}",
            install::gui_archive_name("0.9.4", TARGET)
        )),
        "{error}"
    );
    site.publish("0.9.5", "0.9.5", None, None);
    site.publish_gui("0.9.5", "0.9.0", None);
    let error = site.stage("0.9.5").await.expect_err("wrong GUI version");
    assert_eq!(
        error.to_string(),
        "the 0.9.5 archive's devlanes-gui reports 0.9.0"
    );

    let error = site.stage("0.9.6").await.expect_err("no release");
    assert!(matches!(error, InstallError::Fetch { .. }), "{error}");
    // Nothing reached the install.
    for name in BINARIES {
        assert_eq!(std::fs::read(site.dir.join(name)).expect("old"), b"old");
    }
    assert!(!site.dir.join(GUI_BINARY).exists());
    assert_eq!(Staged::load(&site.home), None);
}

#[test]
fn a_second_install_waits_for_the_lock_or_reports_it_busy() {
    let home = tempfile::tempdir().expect("tempdir");
    let held = Lock::acquire(home.path(), Duration::ZERO).expect("lock");
    let busy = Lock::acquire(home.path(), Duration::from_millis(300)).expect_err("busy");
    assert!(matches!(busy, InstallError::Busy), "{busy}");
    let path = home.path().to_path_buf();
    let waiter =
        std::thread::spawn(move || Lock::acquire(&path, Duration::from_secs(10)).map(drop));
    std::thread::sleep(Duration::from_millis(300));
    drop(held);
    waiter
        .join()
        .expect("join")
        .expect("the waiter got the lock");
}
