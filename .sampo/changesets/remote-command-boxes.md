---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport-command: patch
cargo/devlanes-transport-ssh: patch
---

Added `remote.boxes.<name>.command` to the user config: `devlanes remote connect <name>` runs that command instead of ssh, for containers, pods, WSL distributions and codespaces, and `remote ls` shows `transport: command`.

```yaml
remote:
  boxes:
    acme:
      command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
```
