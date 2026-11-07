---
cargo/devlanes: patch
cargo/devlanes-proto: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes status --mine` showing lane URLs without `https://` and the proxy port.

`status --json` lanes now carry that URL as `url`.
