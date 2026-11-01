---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
cargo/devlanes-proto: minor
cargo/devlanes-resolve: minor
---

Changed lanes to export their project as `DEVLANES_LANE_PROJECT` instead of `DEVLANES_PROJECT`, so a lane's variables no longer pin devlanes commands in another project's folder or checkout to that project.

`DEVLANES_PROJECT` is now only a setting you export yourself, and it wins over the current folder like `-p`. Scripts that read the lane's project from its environment use `DEVLANES_LANE_PROJECT`.
