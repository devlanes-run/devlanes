---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-config: patch
---

Fixed `after: services` tasks running when a service exits within half a second of starting.

Every service now reads `starting` until it has run half a second and its `ready` check passed, so `devlanes status` and `devlanes wait` agree with `devlanes start`.
