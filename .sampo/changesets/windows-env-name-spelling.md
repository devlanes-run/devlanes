---
cargo/devlanes: minor
cargo/devlanes-core: minor
cargo/devlanes-resolve: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
cargo/devlanes-tui: minor
---

Changed variable names on Windows to keep their spelling, so Gradle's `ORG_GRADLE_PROJECT_` properties work.

Names in `env show`, `--explain` and `env export` output keep their spelling too, for example `Path`, so look them up case-insensitively. `exec --env` also replaces a value that waits on a container when the name differs only in case.
