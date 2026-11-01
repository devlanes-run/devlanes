---
cargo/devlanes: minor
---

Removed `--channel` and `--version` from install.sh and `-Channel` and `-Version` from install.ps1; they install the latest release, and `devlanes update --next` switches to the latest build of main.

`DEVLANES_VERSION` is refused too; install a specific release with `npm install -g devlanes@X.Y.Z` or `uv tool install devlanes==X.Y.Z`. When the install directory already holds a devlanes that this user can write to, the scripts run `devlanes update` from it instead. After installing they restart a running daemon into the new version. They stop when a Homebrew, npm, pnpm, Bun, Yarn, uv, pipx or cargo devlanes comes first on `PATH`, unless given `--force` (`-Force`).
