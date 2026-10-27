---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport: patch
cargo/devlanes-tunnel: patch
---

Fixed `devlanes remote connect` reaching past what a box should share: clients no longer receive hidden units or the `sync` folders of hidden services, a port a local program holds on only one loopback address, or on Windows on the wildcard address, is no longer mirrored on any, a box can no longer claim `localhost`, `*.localhost` or the same under `hostnames.tld`, and a box lane whose hostnames overlap a local route through a wildcard is skipped like one with the same name.
