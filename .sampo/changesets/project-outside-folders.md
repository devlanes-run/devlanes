---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `-p` commands such as `devlanes -p acme logs web` failing outside that project's folders.

Outside them, `start`, `stop`, `restart`, `logs` and other one-lane commands act on the project's main lane (lane 0). `lane clean`, `lane new`, `run` and `exec` ask for `--lane`, which `lane new` now takes as the lane whose folder it adds to.
