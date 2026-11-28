# devlanes-gui-tool

The tool side of the GUI protocol, which the `devlanes gui` commands run once they know a GUI service's port. It connects to a GUI host as a tool, picks a window, sends clicks, text, keys, scrolls and resizes and waits for the host to apply them, and writes screenshots as PNG. The `devlanes` binary calls it; it talks to the host through `devlanes-gui-client`.

## Usage

```rust
use std::net::SocketAddr;

use devlanes_gui_proto::{Buttons, Point};
use devlanes_gui_tool::{ToolError, connect, parse_chord, write_png};

async fn save(address: SocketAddr) -> Result<Vec<u8>, ToolError> {
    let mut session = connect(address, "ada").await?;
    let window = session.window(None)?.id;
    session.click(window, Point::new(40, 20), Buttons::LEFT, false).await?;
    session.type_text(window, "hello").await?;
    let chord = parse_chord("ctrl+s").expect("a known chord");
    session.keys(window, &[chord]).await?;
    let frame = session.screenshot(window).await?;
    session.close();
    let mut png = Vec::new();
    write_png(&frame, &mut png)?;
    Ok(png)
}
```

## API

- `connect`, `Session`: a tool connection with its windows, the input steps, `resize` and `screenshot`.
- `Session::window`: the window `--window` names by id or exact title, or the first main window.
- `After`: whether the window is still open and unlocked once the host applied the input.
- `parse_chord`, `known_names`: key names and chords such as `ctrl+s`, as USB HID key codes.
- `write_png`: a BGRA frame written as a PNG, one row at a time.

## Waiting

Every input step ends with the protocol's `Sync`, so a screenshot taken next sees what the app drew by then. `type_text` and `keys` also sync before they send, because toolkits drop keys sent in the same instant as the click that focused a field. The host answers a sync after 3 seconds at the latest, which means the input reached the app, not that the app drew its result. Clicks and scrolls outside the window are refused, text over the protocol's 64 KiB message limit goes in several messages, and a screenshot the host answers with `Failed` (too many at once) is tried once more after 150 ms.

## License

Apache-2.0
