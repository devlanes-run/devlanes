# devlanes-runner-windows

Runs daemon-owned units on Windows and implements the `Runner` and `Unit` traits from `devlanes-core`, as `devlanes-runner-pty` does on Unix. The composition root registers it for the daemon; every Win32 call goes through `devlanes-sys-windows`.

## Usage

```rust
use devlanes_core::{LaunchSpec, StopPolicy, TerminalSize, Unit};
use devlanes_runner_windows::WindowsRunner;

let unit = WindowsRunner::default()
    .with_shell(devlanes_shell_windows::windows_shell().await)
    .spawn_unit(LaunchSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "npm run dev".into()],
        cwd: r"C:\src\acme".into(),
        env: [("PATH".into(), r"C:\Program Files\nodejs;C:\Windows\System32".into())].into(),
        pty: false,
        size: TerminalSize::default(),
    })
    .await?;

let mut output = unit.output();
let report = unit.stop_with_report(StopPolicy::default()).await?;
println!("{:?}, swept: {:?}", report.outcome, report.swept);
```

The `devlanes` binary dispatches `devlanes __console-ctrl <pid> <c|break>` to `console_ctrl_main` before anything else; graceful stops start it.

## API

- `WindowsRunner`: `spawn_unit` returns the concrete `WindowsUnit`, and `Runner::spawn` returns it as `Arc<dyn Unit>`. `with_shell`, `with_stderr`, `with_console_helper`, `with_track_interval` and `with_output_capacity` adjust the defaults.
- `WindowsUnit`: implements `Unit`. Also provides `identity`, `processes`, `listening_ports`, `size`, `set_power`, `terminate`, `exit_report`, `wait_report` and `stop_with_report`.
- `ExitReport`: exit code or signal, stop details and the processes swept after the leader exited.
- `SpawnError`: why a unit did not start, with a stable `code()` such as `shell_missing` or `unsafe_batch_argument`. Converted to `devlanes_core::Error::Io` with a `devlanes_core::SpawnRefusal` source that carries the code.
- `spawn_foreground` and `shell_exit_code`: run a command on this process's console with the same resolution and quoting as units (the test command of `devlanes with`) and map its exit code the way a shell reports it.
- `console_ctrl_main` and `ConsoleHelper`: the helper that sends Ctrl-C or Ctrl-Break to a unit's console.
- `stop_orphan`: stops a unit a previous daemon left running; pass it `WindowsRunner::runtimes()` so it shares the runner's reads of which programs link the MSYS runtime.

## Processes

Each unit starts inside its own job object, so the job's members are its process tree and no process snapshots are taken. The job refuses breakaway, ends a crashing member at once instead of waiting on an error dialog, and does not end its members when the daemon exits, so units outlive a daemon crash and the next daemon finds them as orphans.

The leader gets a hidden console of its own, `NUL` as stdin and pipes for stdout and stderr. The environment is exactly `LaunchSpec::env`: entries such as `=C:` are dropped, and of two names that differ only in case the one sorting last wins.

## Pseudo consoles

A `pty: true` leader runs in a pseudo console (ConPTY) of `LaunchSpec::size`, or `TerminalSize::RENDERED_HEADLESS` (300x40) when that is the default. Input is written to the console and resizes go to it. The inbox console host sends its rendering of the screen, not the bytes the programs wrote, so the unit reports `renders_screen()`. Its own mode switches (`ESC[?9001h`, `ESC[?1004h` and their resets) and window size reports (`ESC[8;<rows>;<columns>t`) are taken out of the output, and the repaint that follows a resize is preceded by `ESC[8;<rows>;<columns>t`, so the daemon's screen model changes size exactly there. A resize asked for while another is being repainted waits for that repaint.

When the unit ends or is stopped, its job is terminated and waited for before the console is closed, which ends the output. The console closes with the daemon too, so `pty: true` units end about 5 seconds after a daemon crash and are not found as orphans.

A bare program name is looked up on the unit's `PATH` with `PATHEXT` (`.COM`, `.EXE`, `.BAT` and `.CMD` only). Batch files run through `<System32>\cmd.exe /e:ON /v:OFF /d /c` with std's escaping, and an argument `cmd.exe` cannot take safely (a line break) is refused. A program relative to a drive's current directory (`C:tool`) and a working directory of 260 characters or more are refused too. A program that links the MSYS2 runtime (`Runtimes` reads its import table) gets every argument in double quotes with `\` and `"` escaped, which that runtime reads back exactly without expanding `*` or `~`; every other program gets the C runtime's quoting, as std's `Command` does.

`/bin/sh`, which `run:` strings resolve to, runs as Git for Windows' `usr\bin\sh.exe` with `MSYS_NO_PATHCONV=1` and `MSYS2_ARG_CONV_EXCL=*`; without it such units fail with `shell_missing`. Git's `usr\bin` is appended to the end of the unit's `PATH`, so scripts find `sleep`, `grep` and `sed` while programs found earlier on `PATH` still win.

## Stopping

| `stop.signal` | `pty: true` | `pty: false` | `pty: false`, an MSYS program runs |
|---|---|---|---|
| `SIGTERM`, `SIGINT` | the Ctrl-C key (`0x03`) | Ctrl-C event | Ctrl-Break event |
| `SIGHUP`, `SIGQUIT`, `SIGUSR1`, `SIGUSR2` | Ctrl-Break event | Ctrl-Break event | Ctrl-Break event |
| `SIGKILL` | terminate the job | terminate the job | terminate the job |

The stop reads the job once and chooses from it; no second event is sent, and a unit that ignores it is terminated at the timeout. A member is an MSYS program when its executable imports `msys-2.0.dll` or `cygwin1.dll` (Git's `usr\bin` tools, MSYS2 and Cygwin programs, whatever they are called), read from the image's import table and cached by path and modification time in `Runtimes`. MSYS starts native programs so that a Ctrl-C event does not reach them, so pipe units with such a member get Ctrl-Break. The typed key reaches them as Ctrl-C, and it is what `wrangler dev`, which reads its console raw, removes its dev registry entry on; a raw-mode program that ignores `0x03` runs until the timeout. `stop_orphan` plans as for a `pty: false` SIGTERM stop.

A graceful stop also posts `WM_CLOSE` to the members' visible windows, then sends one console event through the helper, which the supervisor does not wait for. Once only the console host is left, or at the timeout, the job is terminated; the report says whether the timeout was reached.

The system command interpreter asks "Terminate batch job (Y/N)?" after Ctrl-C while it runs a batch file (a `.cmd` shim such as `npm.cmd`). In a pseudo console, once every live member's image is `<System32>\cmd.exe` (by path, ignoring case, read through the handle the runner holds; a program merely named `cmd.exe` does not count), the runner answers `N`, again every half second while that holds (an answer typed before `cmd.exe` reached the prompt can be lost). `N` lets the batch go on and return its program's code, as `cmd.exe` does in a pipe unit, where it reads end of input at the prompt. A wrong guess is harmless: `N` reaches a `cmd.exe` that is about to exit or continues a batch. The answer is not an escalation.

The report decodes the leader's exit as a Unix wait status reads. When the runner terminated the job while the leader still ran (SIGKILL, or the timeout), the outcome is signal 9. `STATUS_CONTROL_C_EXIT` (`0xC000013A`), what a console event leaves, is signal 2. A leader that links the MSYS runtime reports a death by signal `s` as `s << 8` and cuts a native child's code to 8 bits, so its codes `0x100..=0xFFFF` are signals; Git's `sh` therefore reports SIGINT when the program it ran exits within about 100 ms of the event (an MSYS runtime rule), and the program's code otherwise. Every other code stays a code, including the `512` that just and Git's `bin\bash.exe` launcher pass on from `sh`.

## Idle units

`set_power(Power::Frozen)` suspends every job member except the console host with `NtSuspendProcess`, reading the job again until no new member appears, and keeps each process handle open until the thaw, so every process is resumed once and a reused PID is never touched. `Power::Throttled` is Task Manager's efficiency mode: `IDLE_PRIORITY_CLASS` and EcoQoS on every member, also on members the supervisor's refresh finds later. `Power::Normal` resumes and restores each member's recorded priority class and power throttling (`NORMAL_PRIORITY_CLASS` and Windows' default for members that started while throttled). A process that cannot be resumed or restored keeps its handle, and the next `Normal` tries again. A stop puts a frozen unit back to `Normal` before sending Ctrl-C, Ctrl-Break or `WM_CLOSE`, which suspended threads would never handle. Working sets are not trimmed.

## Limitations

When the leader exits on its own, everything left in its job is ended, for tasks too: a task cannot start a daemon that outlives it, and `ExitReport::left_running` is always empty. Tools that insist on breaking away from the job fail to start their children.

## License

Apache-2.0
