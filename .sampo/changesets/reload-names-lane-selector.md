---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-core: minor
---

Fixed `devlanes reload 1` not saying that `1` is a lane, as `stop 1` does.

`reload` takes globs and `+tag` like `stop`. A service name in `stop` or `reload` also covers a replica the file no longer has, such as `api.3` after `replicas: 2`.
