---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-update: patch
cargo/devlanes-config: patch
---

Changed the dialog after an update to show breaking changes and up to five highlights.

Press `o` in it to open the release notes. `devlanes update` and the install scripts now end with a link to them instead of the full text. Set `updates.highlights: false` in the user config to stop the dialog from opening by itself.
