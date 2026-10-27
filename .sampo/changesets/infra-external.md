---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-state: patch
cargo/devlanes-transport: patch
cargo/devlanes-tui: patch
---

Added `infra.external.<name>` for a server devlanes does not run: every lane lists it as a unit of kind `external`, the proxy routes its `hosts` to its loopback `address`, and machines connected with `devlanes remote connect` mirror it while a lane of the project runs.

A server only you run goes in `devlanes.local.yaml`; see `docs/guides/infra.md#external-ports`:

```yaml
infra:
  external:
    pgadmin:
      address: 127.0.0.1:5050
      hosts: "pgadmin.${{ lane.domain }}"
```
