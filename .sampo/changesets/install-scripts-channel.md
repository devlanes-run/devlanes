---
cargo/devlanes: minor
---

Removed `--channel` from install.sh and `-Channel` from install.ps1; install a stable release, then run `devlanes update --next`.

`DEVLANES_VERSION=next` is refused too. When the install directory already holds a devlanes that this user can write to, the scripts run `devlanes update` from it instead, unless `--version` names an older release. After installing they restart a running daemon into the new version. They stop when a Homebrew, npm, pnpm, Bun, Yarn, uv, pipx or cargo devlanes comes first on `PATH`, unless given `--force` (`-Force`).
