---
cargo/devlanes: minor
---

Removed `--version` from install.sh and `-Version` from install.ps1; they install the latest release, and `--channel next` (`-Channel next`) the latest build of main.

`DEVLANES_VERSION` is refused too; install a specific release with `npm install -g devlanes@X.Y.Z` or `uv tool install devlanes==X.Y.Z`. When the install directory already holds a devlanes that this user can write to, the scripts run `devlanes update` from it instead; `--channel next` always installs by itself. After installing they restart a running daemon into the new version. They stop when a Homebrew, npm, pnpm, Bun, Yarn, uv, pipx or cargo devlanes comes first on `PATH`, unless given `--force` (`-Force`).
