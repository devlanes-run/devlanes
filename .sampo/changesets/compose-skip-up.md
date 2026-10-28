---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-infra-compose: patch
---

Changed `devlanes start` to leave Compose services that already run healthy with unchanged config alone instead of running `docker compose up` again, and to reuse `docker compose config` until a compose or env file changes. After `docker compose pull`, a running container keeps its old image until it is recreated.
