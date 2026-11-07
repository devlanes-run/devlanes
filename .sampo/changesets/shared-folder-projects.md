---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
---

Changed commands run in a folder shared by several projects, such as the repository root, to ask for `-p`.

They used to act on the first registered project. Pass `-p <project>` or run them in a project's folder; `status` and `lane ls` there list every project, and a full lane reference such as `--lane acme@1` still works.
