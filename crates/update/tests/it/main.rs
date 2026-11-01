//! Project-local installs, stable entry paths and the executable to spawn;
//! installing a release in `install`.

mod highlights;
mod install;

use std::path::{Path, PathBuf};

use devlanes_update::{InstallFacts, InstallMethod, Venv};

/// Facts for an executable a project's manifest pins.
fn pinned_by(manifest: &str) -> InstallFacts {
    InstallFacts {
        manifest: Some(PathBuf::from(manifest)),
        ..InstallFacts::default()
    }
}

/// Detects `exe` with `facts` and no cargo home.
fn detect(exe: &str, facts: InstallFacts) -> InstallMethod {
    InstallMethod::detect(Path::new(exe), None, facts)
}

/// A local install with `manifest`.
fn local(manifest: &str) -> InstallMethod {
    InstallMethod::Local {
        manifest: PathBuf::from(manifest),
    }
}

#[test]
fn node_project_dependencies_are_local() {
    for exe in [
        "/p/acme/node_modules/devlanes/bin/devlanes",
        "/p/acme/node_modules/@devlanes/linux-x64/bin/devlanes",
        "/p/acme/node_modules/.pnpm/devlanes@0.9.0/node_modules/devlanes/bin/devlanes",
    ] {
        assert_eq!(
            detect(exe, pinned_by("/p/acme/package.json")),
            local("/p/acme/package.json"),
            "{exe}"
        );
    }
    // Without a manifest that lists devlanes, the path decides as before.
    assert_eq!(
        detect(
            "/p/acme/node_modules/devlanes/bin/devlanes",
            InstallFacts::default()
        ),
        InstallMethod::Npm
    );
    assert_eq!(
        detect(
            "/p/acme/node_modules/.pnpm/devlanes@0.9.0/node_modules/devlanes/bin/devlanes",
            InstallFacts::default()
        ),
        InstallMethod::Pnpm
    );
}

/// A package.json above a global prefix does not make a global install local.
#[test]
fn global_layouts_win_over_a_manifest() {
    let facts = || pinned_by("/Users/a/package.json");
    // npm's Unix prefix layout; Windows keeps them in %APPDATA%\npm.
    #[cfg(unix)]
    assert_eq!(
        detect(
            "/Users/a/.nvm/versions/node/v24.1.0/lib/node_modules/devlanes/bin/devlanes",
            facts()
        ),
        InstallMethod::Npm
    );
    assert_eq!(
        detect(
            "/Users/a/Library/pnpm/global/5/node_modules/.pnpm/devlanes@0.9.0/node_modules/devlanes/bin/devlanes",
            facts()
        ),
        InstallMethod::Pnpm
    );
    assert_eq!(
        detect(
            "/Users/a/.bun/install/global/node_modules/devlanes/bin/devlanes",
            facts()
        ),
        InstallMethod::Bun
    );
    assert_eq!(
        detect(
            "/Users/a/.config/yarn/global/node_modules/devlanes/bin/devlanes",
            facts()
        ),
        InstallMethod::Yarn
    );
}

#[test]
fn python_project_environments_are_local() {
    let with = |venv: Venv, manifest: Option<&str>| InstallFacts {
        venv: Some(venv),
        manifest: manifest.map(PathBuf::from),
        ..InstallFacts::default()
    };
    let exe = "/p/acme/.venv/bin/devlanes";
    assert_eq!(
        detect(exe, with(Venv::Other, Some("/p/acme/pyproject.toml"))),
        local("/p/acme/pyproject.toml")
    );
    assert_eq!(
        detect(exe, with(Venv::Uv, Some("/p/acme/requirements.txt"))),
        local("/p/acme/requirements.txt")
    );
    assert_eq!(
        detect(exe, with(Venv::Other, None)),
        InstallMethod::Pip(PathBuf::from("/p/acme/.venv"))
    );
    // pipx owns its environments, whatever sits next to them.
    assert_eq!(
        detect(exe, with(Venv::Pipx, Some("/p/acme/pyproject.toml"))),
        InstallMethod::Pipx
    );
}

#[test]
fn local_installs_name_their_manifest() {
    #[cfg(unix)]
    let manifest = "/p/acme/package.json";
    #[cfg(windows)]
    let manifest = r"D:\acme\pyproject.toml";
    let method = local(manifest);
    assert_eq!(method.label(), "a project dependency");
    assert_eq!(
        method.describe(None),
        format!("a project dependency, in {manifest}")
    );
    assert_eq!(
        method.upgrade_command("0.9.1"),
        format!("update devlanes in {manifest}")
    );
}

#[cfg(windows)]
#[test]
fn windows_node_project_dependencies_are_local() {
    assert_eq!(
        detect(
            r"D:\acme\node_modules\@devlanes\win32-x64\bin\devlanes.exe",
            pinned_by(r"D:\acme\package.json")
        ),
        local(r"D:\acme\package.json")
    );
    // npm's global packages under %APPDATA% stay npm.
    assert_eq!(
        detect(
            r"C:\Users\a\AppData\Roaming\npm\node_modules\devlanes\node_modules\@devlanes\win32-x64\bin\devlanes.exe",
            pinned_by(r"C:\Users\a\package.json")
        ),
        InstallMethod::Npm
    );
}

#[test]
fn homebrew_entry_is_the_opt_link() {
    assert_eq!(
        InstallMethod::Homebrew.entry(Path::new(
            "/opt/homebrew/Cellar/devlanes/0.9.0/bin/devlanes"
        )),
        PathBuf::from("/opt/homebrew/opt/devlanes/bin/devlanes")
    );
    assert_eq!(
        InstallMethod::Homebrew.entry(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/devlanes/0.9.0/bin/devlanes"
        )),
        PathBuf::from("/home/linuxbrew/.linuxbrew/opt/devlanes/bin/devlanes")
    );
    assert_eq!(
        InstallMethod::Homebrew.entry(Path::new("/opt/elsewhere/devlanes")),
        PathBuf::from("/opt/elsewhere/devlanes")
    );
}

#[test]
fn pnpm_entry_is_the_package_link() {
    assert_eq!(
        InstallMethod::Pnpm.entry(Path::new(
            "/Users/a/Library/pnpm/global/5/node_modules/.pnpm/devlanes@0.9.0/node_modules/devlanes/bin/devlanes"
        )),
        PathBuf::from("/Users/a/Library/pnpm/global/5/node_modules/devlanes/bin/devlanes")
    );
    // The platform package's binary has no link that follows upgrades.
    let platform = Path::new(
        "/Users/a/Library/pnpm/global/5/node_modules/.pnpm/@devlanes+darwin-arm64@0.9.0/node_modules/@devlanes/darwin-arm64/bin/devlanes",
    );
    assert_eq!(InstallMethod::Pnpm.entry(platform), platform);
}

#[cfg(windows)]
#[test]
fn pnpm_entry_on_windows() {
    assert_eq!(
        InstallMethod::Pnpm.entry(Path::new(
            r"C:\Users\a\AppData\Local\pnpm\global\5\node_modules\.pnpm\devlanes@0.9.0\node_modules\devlanes\bin\devlanes.exe"
        )),
        PathBuf::from(
            r"C:\Users\a\AppData\Local\pnpm\global\5\node_modules\devlanes\bin\devlanes.exe"
        )
    );
}

/// A script install runs the executable's own name in its directory, which
/// a person may have renamed.
#[cfg(unix)]
#[test]
fn script_entry_keeps_the_executable_name() {
    let script = InstallMethod::Script(PathBuf::from("/Users/a/.local/bin"));
    assert_eq!(
        script.entry(Path::new("/Users/a/.local/bin/devlanes")),
        PathBuf::from("/Users/a/.local/bin/devlanes")
    );
    assert_eq!(
        script.entry(Path::new("/Users/a/.local/bin/dl")),
        PathBuf::from("/Users/a/.local/bin/dl")
    );
}

/// Linux names a replaced executable `<path> (deleted)`; the entry is the path.
#[cfg(target_os = "linux")]
#[test]
fn script_entry_drops_the_deleted_suffix() {
    assert_eq!(
        InstallMethod::Script(PathBuf::from("/home/a/.local/bin"))
            .entry(Path::new("/home/a/.local/bin/devlanes (deleted)")),
        PathBuf::from("/home/a/.local/bin/devlanes")
    );
}

#[cfg(windows)]
#[test]
fn script_entry_keeps_the_executable_name() {
    let dir = PathBuf::from(r"C:\Users\a\AppData\Local\Programs\devlanes");
    let script = InstallMethod::Script(dir.clone());
    assert_eq!(
        script.entry(&dir.join("devlanes.exe")),
        dir.join("devlanes.exe")
    );
    assert_eq!(script.entry(&dir.join("dl.exe")), dir.join("dl.exe"));
}

/// A program at `dir/name` whose `--version` prints `stdout` after `sleep`.
#[cfg(unix)]
fn fake_program(dir: &Path, name: &str, stdout: &str, sleep: std::time::Duration) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nsleep {}\nprintf '%s\\n' '{stdout}'\n",
            sleep.as_secs()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
    path
}

#[cfg(windows)]
fn fake_program(dir: &Path, name: &str, stdout: &str, sleep: std::time::Duration) -> PathBuf {
    use devlanes_testing::fake_tool::{Behaviour, install};
    install(
        dir,
        name,
        &Behaviour::new().sleep(sleep).stdout(format!("{stdout}\n")),
    )
    .expect("fake-tool")
}

/// The entry is spawned only when it is there and says it is devlanes, in
/// time; otherwise the running executable is.
#[test]
fn the_entry_is_spawned_only_when_it_runs_devlanes() {
    use std::time::Duration;
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("running");
    let quick = Duration::ZERO;
    let good = fake_program(dir.path(), "good", "devlanes 0.9.1", quick);
    let other = fake_program(dir.path(), "other", "acme 0.9.1", quick);
    let slow = fake_program(dir.path(), "slow", "devlanes 0.9.1", Duration::from_secs(3));
    let patient = Duration::from_secs(30);
    assert_eq!(
        devlanes_update::version_of(&good, patient).as_deref(),
        Some("0.9.1")
    );
    assert_eq!(devlanes_update::version_of(&other, patient), None);
    assert_eq!(
        devlanes_update::version_of(&slow, Duration::from_millis(300)),
        None
    );
    let missing = dir.path().join("missing");
    assert_eq!(devlanes_update::version_of(&missing, patient), None);
    assert_eq!(devlanes_update::entry_or(good.clone(), exe.clone()), good);
    assert_eq!(devlanes_update::entry_or(other, exe.clone()), exe);
    assert_eq!(devlanes_update::entry_or(missing, exe.clone()), exe);
}

/// Methods that upgrade in place run from the same path.
#[test]
fn other_entries_are_the_executable() {
    let exe = Path::new("/srv/devlanes/bin/devlanes");
    for method in [
        InstallMethod::Npm,
        InstallMethod::Bun,
        InstallMethod::Yarn,
        InstallMethod::CargoInstall,
        InstallMethod::Uv,
        InstallMethod::Pipx,
        InstallMethod::Pip(PathBuf::from("/srv/devlanes")),
        local("/srv/package.json"),
        InstallMethod::Source,
    ] {
        assert_eq!(method.entry(exe), exe, "{method:?}");
    }
}

/// A test binary is a source build, so it spawns itself.
#[test]
fn spawn_exe_is_this_test_binary() -> std::io::Result<()> {
    let spawn = devlanes_update::spawn_exe()?;
    assert!(spawn.is_file(), "{}", spawn.display());
    assert_eq!(spawn, dunce::canonicalize(std::env::current_exe()?)?);
    Ok(())
}
