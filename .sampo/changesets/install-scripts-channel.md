---
cargo/devlanes: minor
---

Removed `--channel` from install.sh and `-Channel` from install.ps1; install a stable release, then run `devlanes update --next`.

The scripts now hand a devlanes already in the install directory to `devlanes update`.
