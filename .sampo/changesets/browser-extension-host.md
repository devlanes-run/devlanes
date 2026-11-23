---
cargo/devlanes: patch
cargo/devlanes-browser-host: patch
---

Added the native host for the devlanes browser extension, which `devlanes setup` registers for Chrome, Chromium, Brave and Edge.

The extension doesn't keep the daemon running, so the TUI's exit summary may say the daemon stops soon while the popup is open.
