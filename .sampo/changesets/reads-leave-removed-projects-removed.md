---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-core: minor
cargo/devlanes-cli: patch
---

Changed `url ls`, `debug top`, `logs` and other reads to leave a project removed by `lane clean` removed.

Only `start`, `restart`, `reload`, `run`, `exec`, `lane new`, `infra`, `env`, `url`, `status` and `lane ls` add the folder's project. Other reads in a project nobody started say so; `stop` and `lane gc` do nothing there. A `devlanes.yaml` that does not load now shows its error in `status` and `url ls`. `config check` warns about hostnames before the first start.
