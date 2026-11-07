---
cargo/devlanes: patch
cargo/devlanes-core: patch
cargo/devlanes-runner-windows: patch
---

Changed the default Windows stop (`stop.signal: SIGTERM`) to send Ctrl-C, so Node programs such as `wrangler dev` run their shutdown code.

Units where Git's `sh` or `bash` runs, and `run:` strings, still get Ctrl-Break.
