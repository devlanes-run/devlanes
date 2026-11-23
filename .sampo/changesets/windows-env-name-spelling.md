---
cargo/devlanes: minor
cargo/devlanes-core: minor
cargo/devlanes-resolve: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
cargo/devlanes-tui: minor
cargo/devlanes-env-direnv: patch
cargo/devlanes-env-mise: patch
---

Changed variable names on Windows to keep the case they are written in, so Gradle's `ORG_GRADLE_PROJECT_` properties work.

Names in `env show`, `--explain` and `env export` output keep their spelling (for example `Path`), so look them up case-insensitively. Restart the daemon after updating so `${{ env.X }}` in infra fields finds any spelling.
