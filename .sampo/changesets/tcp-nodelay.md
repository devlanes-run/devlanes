---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-loopback: patch
cargo/devlanes-proxy: patch
---

Fixed small writes through box ports and the proxy, such as WebSocket messages, sometimes waiting about 40 ms before they were sent.
