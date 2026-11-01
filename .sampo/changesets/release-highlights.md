---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-update: patch
cargo/devlanes-config: patch
---

Changed the dialog the first TUI shows after an update to list breaking changes and up to five highlights, with `o` opening the release page.

`devlanes update` and the install scripts now end with a link to the release page instead of the full notes. Set `updates.highlights: false` in the user config to stop the dialog from opening by itself.
