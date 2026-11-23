---
cargo/devlanes: patch
cargo/devlanes-browser-host: patch
---

Added browser extension support to `devlanes setup` for Chrome, Chromium, Brave and Edge.

The extension doesn't keep the daemon running, so the daemon can stop while the popup is open.
