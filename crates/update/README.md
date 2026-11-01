# devlanes-update

How this devlanes binary was installed, the command that updates it, the stable path that runs the install after an update, release versions, release checks and notes, installing a release, and the release notices clients show. The `devlanes` binary, the TUI, the client and the daemon use it; it depends on `devlanes-core` (the user's home) and `devlanes-version`, and runs `curl`, `tar` and package managers.

## Usage

```rust
use devlanes_update::{InstallMethod, NoticeLog, update_command, update_key, version::is_newer};

let method = InstallMethod::current();
println!("installed with {}", method.describe(None));
let latest = "0.9.1";
if is_newer(latest, devlanes_version::VERSION) {
    // `devlanes update`, or the install's own command when devlanes can't install it.
    println!("{}", update_command(&method, latest));
}

// The daemon to start: Homebrew's opt link rather than a Cellar path.
let daemon_exe = devlanes_update::spawn_exe();

let log = NoticeLog::load(std::path::Path::new("/home/acme/.devlanes"));
let due = log.due(&update_key(latest), devlanes_update::now_ms());
```

## API

- `InstallMethod::current()` reads how this binary was installed; `InstallMethod::detect` does the same from a path and `InstallFacts`, without touching the filesystem.
- `update_command` is the command a notice names: `devlanes update` where devlanes installs releases itself, else `InstallMethod::upgrade_command`, the install's own command for a person to run. `describe` names the install for `doctor`.
- `InstallMethod::entry` is the path that runs this install after an update, and `spawn_exe()` is the executable to start for a daemon or a login service.
- `InstallMethod::current_with_entry()` is the method and entry path of the running binary, for `doctor`.
- `version::Version` and `version::is_newer` order release versions by semver; `version::bump` says whether a newer release is a patch or a minor one. `version::running` is the version this binary reports and updates from.
- `check::Source::for_method` picks the registry an install reads its latest version from (`GitHub`, `Npm`, `PyPi`, `Homebrew`), `check::latest` reads it with `curl`, and `check::release_notes` reads a release's `CHANGELOG.md`. `check::Urls` holds the addresses, which tests replace; a `next` build (`check::is_next_build`) reads the `next` prerelease's manifest instead of the latest stable release's.
- `notes::parse` reads devlanes' `CHANGELOG.md` into releases and entries, and `notes::between` picks the releases after one version up to another. It uses only the standard library, so the `devlanes` build script embeds the changelog with it. `notes::WhatsNew` is what a "What's new" shows, minor entries first.
- `whats_new_on_start` decides whether opening devlanes shows "What's new" from the version `NoticeLog` saw last, and `claim_whats_new` applies it under the update lock so only one TUI shows it.
- `install::Plan::new` says how a release gets installed over an install: the release archive, a package manager (argv, never a shell), or a command for a person. `install::stage` downloads, checks and unpacks the archive under `$DEVLANES_HOME/update/`, `install::apply` renames it into place, and `install::run_package_manager` runs the package manager. `install::Lock` keeps it to one install at a time. `install::removal_commands` are the package manager's remove commands for `devlanes uninstall`, run with `install::run_commands`.
- `NoticeLog` records when each notice was shown or dismissed (`notices.json`); `update_line`, `ready_line` and `skew_line` are the notices.
- `remove_old_copies` (Windows) deletes the `devlanes.exe.old-*` files `install.ps1` and updates leave.
- `retain_own_exe` (Windows) keeps a hard link to the running executable in `$DEVLANES_HOME/run/exe/`, so a package manager can replace `devlanes.exe` while it runs.

## Test releases

Debug builds read every release address under `DEVLANES_UPDATE_BASE_URL` when it is set (`file://` included), laid out like GitHub's `releases/download/v<version>/`, and the daemon checks for releases there even though debug builds otherwise never check. `DEVLANES_DEBUG_VERSION` makes a debug build report and update from that version instead of its own, so a test can publish a release of the same binary and have `devlanes update` install it, which takes only a newer release; the staged release and the binary `update` hands over to run without it. Release builds ignore both.

## Project dependencies

devlanes installed by a project (a `node_modules/devlanes` that the project's `package.json` lists, or a Python environment next to a `pyproject.toml` or `requirements*.txt`) is `InstallMethod::Local`. It has no release source, so nothing checks for its releases, and its update advice points at that manifest, since a global command would not change the version the project pins.

## Stable paths

A long-running TUI or daemon outlives updates. Homebrew's Cellar path names the version and `brew cleanup` removes it, pnpm's store path names the version too, and Linux reports a replaced executable as `<path> (deleted)`. `spawn_exe` starts the install's stable path instead (`<prefix>/opt/devlanes/bin/devlanes` for Homebrew), so a restart runs the binary installed now.

## License

Apache-2.0
