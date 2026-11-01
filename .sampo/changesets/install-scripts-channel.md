---
cargo/devlanes: minor
---

Removed `--channel` from install.sh and `-Channel` from install.ps1; install a stable release, then run `devlanes update --next`.

`DEVLANES_VERSION=next` is refused too. The scripts now hand a devlanes already in the install directory to `devlanes update`, and stop when a Homebrew, npm, uv, pipx or cargo devlanes comes first on `PATH` unless given `--force` (`-Force`).
