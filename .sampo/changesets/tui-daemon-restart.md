---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
---

Changed the TUI while the daemon restarts, stops or does not answer: every open TUI shows the daemon page with the live daemon log and returns to your view once the daemon answers, and a restart from the TUI starts the lanes that ran again with their usual progress instead of staying on "daemon restarting…" until every lane is ready.

The daemon now sends a `daemon.stopping` event when it begins to shut down. Confirm dialogs no longer repeat their question as a title.
