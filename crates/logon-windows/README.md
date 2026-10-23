# devlanes-logon-windows

Starts the devlanes daemon at logon on Windows through the current user's `Run` key. `devlanes daemon install` and `uninstall` call it, and `doctor` reads its status; it writes the registry through `devlanes-sys-windows`.

## Usage

```rust
use devlanes_logon_windows::{Change, Logon};

fn install() -> std::io::Result<()> {
    let logon = Logon::user();
    let installed = logon.install(&std::env::current_exe()?)?;
    if installed.change == Change::Installed {
        println!("added {}", logon.location());
    }
    let status = logon.status()?;
    println!("{:?}, enabled: {}", status.command, status.enabled);
    Ok(())
}
```

## API

- `Logon::user()`: the real entry, `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\devlanes`.
- `Logon::at(run, approved, name)`: the same under other `HKCU` subkeys, for tests.
- `Logon::install`, `status`, `uninstall`: write, read and remove the entry.
- `command(exe)`: the value written, `"<exe>" daemon run`.

## Task Manager

Task Manager's Startup apps page keeps its on/off switch in `...\Explorer\StartupApproved\Run`, as a binary record whose odd first byte means off. `status` reports it, `install` turns the entry back on, and `uninstall` removes the record with the value.

## No console window

The Run key starts `devlanes.exe` without a console. The binary's manifest sets `consoleAllocationPolicy` to `detached` (Windows 11 24H2 and later), so Windows opens no window, and `daemon run` then gives itself a hidden console that the programs it starts share.

The Run key carries no environment, so the daemon it starts uses the default home; `daemon install` refuses a different `DEVLANES_HOME`.

## License

Apache-2.0
