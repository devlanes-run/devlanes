# devlanes-update

How this devlanes binary was installed, the command that upgrades it, the stable path that runs the install after an upgrade, release versions, and the release notices clients show. The `devlanes` binary, the TUI, the client and the daemon use it; it depends only on `devlanes-core` (the user's home) and `devlanes-version`.

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
- `version::Version` and `version::is_newer` order release versions by semver.
- `NoticeLog` records when each notice was shown or dismissed (`notices.json`); `update_line` and `skew_line` are the notices.
- `remove_old_copies` (Windows) deletes the `devlanes.exe.old-*` files `install.ps1` leaves.

## Project dependencies

devlanes installed by a project (a `node_modules/devlanes` that the project's `package.json` lists, or a Python environment next to a `pyproject.toml` or `requirements*.txt`) is `InstallMethod::Local`. Its upgrade advice points at that manifest, since a global command would not change the version the project pins.

## Stable paths

A long-running TUI or daemon outlives upgrades. Homebrew's Cellar path names the version and `brew cleanup` removes it, pnpm's store path names the version too, and Linux reports a replaced executable as `<path> (deleted)`. `spawn_exe` starts the install's stable path instead (`<prefix>/opt/devlanes/bin/devlanes` for Homebrew), so a restart runs the binary installed now.

## License

Apache-2.0
