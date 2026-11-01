# devlanes-update

How this devlanes binary was installed, the command that upgrades it, the stable path that runs the install after an upgrade, release versions, release checks and notes, installing a release, and the release notices clients show. The `devlanes` binary, the TUI, the client and the daemon use it; it depends on `devlanes-core` (the user's home) and `devlanes-version`, and runs `curl`, `tar` and package managers.

## Usage

```rust
use devlanes_update::{InstallMethod, NoticeLog, update_key, version::is_newer};

let method = InstallMethod::current();
println!("installed with {}", method.describe(None));
if is_newer("0.9.1", devlanes_version::VERSION) {
    println!("{}", method.upgrade_command("0.9.1"));
}

// The daemon to start: Homebrew's opt link rather than a Cellar path.
let daemon_exe = devlanes_update::spawn_exe();

let log = NoticeLog::load(std::path::Path::new("/home/acme/.devlanes"));
let due = log.due(&update_key("0.9.1"), devlanes_update::now_ms());
```

## API

- `InstallMethod::current()` reads how this binary was installed; `InstallMethod::detect` does the same from a path and `InstallFacts`, without touching the filesystem.
- `InstallMethod::upgrade_command` is the command to show for an upgrade; `describe` names the install for `doctor`.
- `InstallMethod::entry` is the path that runs this install after an upgrade, and `spawn_exe()` is the executable to start for a daemon or a login service.
- `InstallMethod::current_with_entry()` is the method and entry path of the running binary, for `doctor`.
- `version::Version` and `version::is_newer` order release versions by semver; `version::bump` says whether a newer release is a patch or a minor one.
- `check::Source::for_method` picks the registry an install reads its latest version from (`GitHub`, `Npm`, `PyPi`, `Homebrew`), `check::latest` reads it with `curl`, and `check::release_notes` reads a release's `CHANGELOG.md`. `check::Urls` holds the addresses, which tests replace.
- `notes::parse` reads devlanes' `CHANGELOG.md` into releases and entries, and `notes::between` picks the releases after one version up to another. It uses only the standard library.
- `install::Plan::new` says how a release gets installed over an install: the release archive, a package manager (argv, never a shell), or a command for a person. `install::stage` downloads, checks and unpacks the archive under `$DEVLANES_HOME/update/`, `install::apply` renames it into place, and `install::run_package_manager` runs the package manager. `install::Lock` keeps it to one install at a time.
- `NoticeLog` records when each notice was shown or dismissed (`notices.json`); `update_line`, `ready_line` and `skew_line` are the notices, and `update_command` is the command they name (`devlanes update` where devlanes installs releases itself).
- `remove_old_copies` (Windows) deletes the `devlanes.exe.old-*` files `install.ps1` and updates leave.
- `retain_own_exe` (Windows) keeps a hard link to the running executable in `$DEVLANES_HOME/run/exe/`, so a package manager can replace `devlanes.exe` while it runs.

## Test releases

Debug builds read every release address under `DEVLANES_UPDATE_BASE_URL` when it is set (`file://` included), laid out like GitHub's `releases/download/v<version>/`, and the daemon checks for releases there even though debug builds otherwise never check. Release builds ignore it.

## Project dependencies

devlanes installed by a project (a `node_modules/devlanes` that the project's `package.json` lists, or a Python environment next to a `pyproject.toml` or `requirements*.txt`) is `InstallMethod::Local`. Its upgrade advice points at that manifest, since a global command would not change the version the project pins.

## Stable paths

A long-running TUI or daemon outlives upgrades. Homebrew's Cellar path names the version and `brew cleanup` removes it, pnpm's store path names the version too, and Linux reports a replaced executable as `<path> (deleted)`. `spawn_exe` starts the install's stable path instead (`<prefix>/opt/devlanes/bin/devlanes` for Homebrew), so a restart runs the binary installed now.

## License

Apache-2.0
