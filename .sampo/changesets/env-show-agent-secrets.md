---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-mcp: minor
---

Changed `devlanes env show` in agent output and the MCP `env_show` tool to hide secret-looking values.

`devlanes mcp` no longer runs `env export`. In a shell, `env show --output plain` and `eval "$(devlanes env export)"` still print real values, and `exec` or `with` pass them to a command. A `--read-only` MCP server can no longer read secret values.
