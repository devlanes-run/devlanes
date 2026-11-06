# devlanes-mcp

The MCP server behind `devlanes mcp`. `bins/devlanes` starts it on stdin and stdout; each tool call becomes a devlanes command line, which this crate checks with the `devlanes-cli` parser and runs as a child process of the devlanes binary with `--output agent`.

## Usage

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = devlanes_mcp::Config::from_process(std::env::current_exe()?)?;
    config.read_only = true;
    config.ui = Some(devlanes_mcp::UiResource {
        uri: devlanes_mcp_ui::URI,
        mime: devlanes_mcp_ui::MIME_TYPE,
        html: devlanes_mcp_ui::html,
    });
    devlanes_mcp::serve(config).await?;
    Ok(())
}
```

## API

- `serve(config)`: serves MCP on stdio until the client closes stdin or the process gets SIGTERM, SIGINT or SIGHUP.
- `Server::new(config)` and `serve_on(server, reader, writer, stop)`: the rmcp handler and the loop around it, for serving over another stream (the tests use an in-memory pipe).
- `Config`: the executable to run, `--read-only`, `--lane`, `--path`, the environment and directory of the calls.
- `shell_env::resolve` and `shell_env::print_payload`: read the login shell's environment, and the `devlanes __shell-env MARK` side that prints it.
- `UiResource`: the MCP Apps HTML (`ui://devlanes/lane.html`), given by the binary from `devlanes-mcp-ui`; `UiResource::placeholder()` is a minimal page for tests.

## Tools

Each command in the curated list in `src/tools.rs` (`status`, `start`, `exec`, `logs`, `stop`, `debug_why`, `lane_clean` and the others) gets a tool generated from the `devlanes-cli` clap definition: the name is the subcommand path joined with `_`, the description is the command's help, and the input schema has a property per argument and option, with its type, possible values, default and help. A call's arguments become a command line again (path, arguments in clap's order, `--flag=value`, the values after `--` last), so a new flag on one of these commands reaches its tool without a change here. The `devlanes` tool takes `{args: string[], cwd?: string}` for every other command. Tools carry `readOnlyHint` or `destructiveHint` from `Command::is_read_only`; `--read-only` offers only the read-only tools and limits `devlanes` to read-only commands. Commands that need a terminal, and follows without `--timeout`, are refused. Blocking commands without `--timeout` get one a little under the host's own tool timeout, chosen from the client's name, or 60 s for a host it does not know. Clients that advertise the `io.modelcontextprotocol/ui` extension also see `app_snapshot`, `app_logs` and `app_action`, marked `_meta.ui.visibility: ["app"]`, and the cards resource.

A result's text is the command's stdout and stderr; `structuredContent` holds `{command, args, cwd, exit_code, view, card, data, read_only}`, where `data` is what `--output json` would have printed (written by the child to the file `DEVLANES_JSON_OUT` names, with `{"error": ...}` for a failure), `view` names the card for it, `card` numbers the cards about one lane (1, 2, 3... per lane while the server runs, null for other results; `app_snapshot` returns the newest in `newest_card`) and `read_only` says whether the server runs with `--read-only`. Above 256 KiB of JSON, `data` is null and `data_truncated` is `true`.

## Child processes

Nothing connects to the daemon when the server starts. Each call spawns the devlanes binary with stdin closed and stdout and stderr piped, so the protocol stream stays clean. The first run sets `DEVLANES_ENV_FROM_LANE=1`, so a lane a shell started keeps that shell's environment; when the daemon answers `needs_env`, the call runs again with the environment of the user's login shell over the server's (`shell_env`, read once per server: `$SHELL` on Unix unless `SHLVL` is above 0, PowerShell on Windows only with `DEVLANES_SHELL_ENV=pwsh` or `powershell`), or with the server's alone when that fails, and the result says which. When the environment names no agent, `AI_AGENT` comes from the client's name.

Each child leads a process group of its own, and on Windows also runs in a job that ends it when the server dies. Cancelling a call interrupts the child as Ctrl-C would and, 5 s later, kills it and what is left in its group or job; the daemon keeps doing what was asked. When the client closes stdin or the server is told to stop, running commands get 1 s instead, since hosts kill the server soon after.

## Codex on Windows

A shell inside the Codex sandbox on Windows cannot open the daemon's pipe, even when the daemon already runs. Codex starts MCP servers outside its sandbox, so there `devlanes mcp` is how Codex reaches devlanes.

## Protocol versions

rmcp negotiates every revision it knows: the `initialize` handshake up to 2025-11-25, and `server/discover` with per-request metadata from 2026-07-28. Consent questions are `elicitation/create` requests before 2026-07-28 and an `InputRequiredResult` the client answers by calling again from then on. Clients without elicitation get "ask the user, then rerun with --yes".

## License

Apache-2.0
