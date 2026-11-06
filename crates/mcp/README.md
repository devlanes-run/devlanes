# devlanes-mcp

The MCP server behind `devlanes mcp`. `bins/devlanes` starts it on stdin and stdout; each tool call names a devlanes command line, which this crate checks with the `devlanes-cli` parser and runs as a child process of the devlanes binary with `--output agent`.

## Usage

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = devlanes_mcp::Config::from_process(std::env::current_exe()?)?;
    config.read_only = true;
    config.ui = Some(devlanes_mcp::UiResource::placeholder());
    devlanes_mcp::serve(config).await?;
    Ok(())
}
```

## API

- `serve(config)`: serves MCP on stdio until the client leaves.
- `Server::new(config)`: the rmcp handler, for serving over another transport (the tests use an in-memory pipe).
- `Config`: the executable to run, `--read-only`, `--lane`, `--path`, the environment and directory of the calls.
- `UiResource`: the MCP Apps HTML (`ui://devlanes/lane.html`), given by the binary; `UiResource::placeholder()` until the cards exist.

## Tools

`devlanes` runs any command and `devlanes_read` only those `Command::is_read_only` accepts; both take `{args: string[], cwd?: string}`. `--read-only` offers only `devlanes_read`. Commands that need a terminal, and follows without `--timeout`, are refused. Blocking commands without `--timeout` get one a little under the host's own tool timeout, chosen from the client's name. Clients that advertise the `io.modelcontextprotocol/ui` extension also see `app_snapshot`, `app_logs` and `app_action`, marked `_meta.ui.visibility: ["app"]`, and the cards resource.

A result's text is the command's stdout and stderr; `structuredContent` holds `{command, args, cwd, exit_code, view, data}`, where `data` is what `--output json` would have printed (written by the child to the file `DEVLANES_JSON_OUT` names, with `{"error": ...}` for a failure) and `view` names the card for it.

## Child processes

Nothing connects to the daemon when the server starts. Each call spawns the devlanes binary with stdin closed and stdout and stderr piped, so the protocol stream stays clean. The first run sets `DEVLANES_ENV_FROM_LANE=1`, so a lane a shell started keeps that shell's environment; when the daemon answers `needs_env`, the call runs again with the server's environment and the result says so. When the environment names no agent, `AI_AGENT` comes from the client's name. Cancelling a call kills its child; the daemon keeps doing what was asked.

## Codex on Windows

A shell inside the Codex sandbox on Windows cannot open the daemon's pipe, even when the daemon already runs. Codex starts MCP servers outside its sandbox, so there `devlanes mcp` is how Codex reaches devlanes.

## Protocol versions

rmcp negotiates every revision it knows: the `initialize` handshake up to 2025-11-25, and `server/discover` with per-request metadata from 2026-07-28. Consent questions are `elicitation/create` requests before 2026-07-28 and an `InputRequiredResult` the client answers by calling again from then on. Clients without elicitation get "ask the user, then rerun with --yes".

## License

Apache-2.0
