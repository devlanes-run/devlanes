---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport-command: patch
cargo/devlanes-transport-ssh: patch
---

Added `remote.boxes.<name>.command` to the user config, so `devlanes remote connect <name>` reaches a container, pod, WSL distribution or codespace by running a command such as `docker exec` instead of SSH.

```yaml
remote:
  boxes:
    acme:
      command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
```

See `docs/guides/remote-command.md`.
