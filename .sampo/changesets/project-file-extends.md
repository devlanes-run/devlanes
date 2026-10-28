---
cargo/devlanes: patch
cargo/devlanes-config: patch
---

Added a top-level `extends` key that builds a project file on one or more others, each merged with its own `.local.yaml`, so a `devlanes.prod.yaml` can reuse `devlanes.yaml` and change only what differs.

```yaml
# devlanes.prod.yaml
extends: devlanes.yaml
project: acme-prod
profiles:
  default: { include: ["*"], run_scripts: { dev: start } }
```
