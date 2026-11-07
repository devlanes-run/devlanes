---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
---

Changed `devlanes stop --mine` without units to stop only your units in the current project.

Use `-p PROJECT` for another project, or run it outside any project to cover every project. The output names the command when your units still run elsewhere, and `status --mine` lists the same units.
