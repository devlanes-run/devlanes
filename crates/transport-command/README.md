# devlanes-transport-command

The command `Transport` for `devlanes remote connect`: it runs a local command, such as `docker exec -i acme devlanes tunnel --stdio`, and speaks the remote protocol over its standard input and output. The binary's composition root uses it for boxes with `remote.boxes.<name>.command` in the user config, and `devlanes-transport-ssh` runs its tunnels through it.

## Usage

```rust
use devlanes_transport::{Session, SessionOptions};
use devlanes_transport_command::{CommandOptions, CommandTransport};

async fn connect() -> Result<Session, Box<dyn std::error::Error>> {
    let transport = CommandTransport::new(
        ["docker", "exec", "-i", "acme", "devlanes", "tunnel", "--stdio"],
        CommandOptions::new("acme"),
    );
    let opened = transport.open().await?;
    println!("before the tunnel: {}", String::from_utf8_lossy(&opened.header));
    drop(opened);
    Ok(Session::connect(&transport, SessionOptions::new("devlanes 0.1.0")).await?)
}
```

## API

- `CommandTransport::new(argv, options)`: a `Transport` that starts `argv` for each connect; `open` also returns what the command printed before the tunnel's preamble.
- `CommandOptions`: the box name and how messages call the process, the preamble wait (`COMMAND_WAIT`, 120 s), an optional `Ready` line with its own wait, environment changes, `connected_before` and a `stop` token that ends every command started with it.
- `CommandError::is_permanent`: separates a command that cannot work as given from one that may work on the next try.
- `CommandStream`: the command's stdout and stdin; at end of file it reports the exit status and stderr tail.
- `run(argv, options, limit)`: runs a short command to its end, for an SSH probe.

## Behaviour

A connect fails for good when the program cannot start, when the box speaks another protocol, and, before the first connection, when the command exits with 127 (a program it runs was not found, usually devlanes inside the box). Silence or more than 64 KiB of output before the preamble is permanent only before the first connection. Exit status 75 (`devlanes_transport::EXIT_DAEMON_STOPPED`) is `CommandError::DaemonStopped`, temporary and with the fix for `remote.start_daemon: false`. Other failures are temporary and carry the exit status, the last 4 KiB of stderr and what the command printed.

The command gets this process's environment and no terminal. A prompt it shows has nobody to answer it, so it fails or hangs until the preamble wait ends.

On Windows each command runs in a kill-on-close job, so whatever it starts ends with it, and with the daemon. It shares the daemon's hidden console, or gets a windowless one when there is none. A program without a directory is looked up on `PATH` as `.exe` or `.com`; batch files need `cmd /c`.

`src/bin/fake-box.rs` is a stand-in box for this crate's tests; it is not shipped.

## License

Apache-2.0
