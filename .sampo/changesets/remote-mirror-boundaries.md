---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport: patch
cargo/devlanes-tunnel: patch
---

Fixed `devlanes remote connect` sending clients the names and `sync` folders of `hidden` services and containers, mirroring ports a local program holds on one loopback or the wildcard address, and letting a box claim `localhost`, `*.localhost` or names under a local wildcard.
