# devlanes-console-windows

The Windows console side of the devlanes CLI: prompts and `attach`. Where Unix opens `/dev/tty`, switches termios modes and listens for SIGWINCH, the binary calls this crate, which reads `CONIN$` with console modes through `devlanes-sys-windows` and polls the window size. On other platforms the crate is empty.

## Usage

```rust
use devlanes_console_windows::{ConsoleText, RawGuard, Resizes, read_input, read_tty};

fn confirm() -> bool {
    read_tty("Remove lane 2? [y/N]", "confirm")
        .is_some_and(|line| line.trim().eq_ignore_ascii_case("y"))
}

async fn attach(input_tx: tokio::sync::mpsc::Sender<Vec<u8>>) -> std::io::Result<()> {
    let _raw = RawGuard::enable(true)?;
    std::thread::spawn(move || read_input(&input_tx));
    let mut resizes = Resizes::new();
    let mut text = ConsoleText::stdout();
    while resizes.recv().await.is_some() {
        let _frame = text.convert(b"unit output");
    }
    Ok(())
}
```

## API

- `read_tty(prompt, kind)`: one answer from the console, `None` when nobody can type there, for Ctrl-C and for a Ctrl-Z line.
- `RawGuard::enable`: raw console input with VT sequences for `attach`, restored on drop.
- `read_input`: the raw console's keys as UTF-8, until input ends.
- `ConsoleText`: unit output made valid UTF-8 for a console, unchanged for files and pipes.
- `Resizes`: window size changes, polled every 250 ms.

## mintty

Git Bash's mintty passes no typed keys to Windows programs. There a prompt is shown on stderr and anything but a notice is refused, and `attach` refuses to start; Windows Terminal, a console window or `winpty` work.

## License

Apache-2.0
