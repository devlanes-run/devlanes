# devlanes-gui-tool

What the `devlanes gui` commands do once they know a GUI service's port: connect to its GUI host as a tool, pick a window, send clicks, text, keys, scrolls and resizes and wait for the host to apply them, and take screenshots as PNG. The `devlanes` binary calls it; it talks to the host through `devlanes-gui-client`.

## Usage

```rust
use std::net::SocketAddr;

use devlanes_gui_proto::{Buttons, Point};
use devlanes_gui_tool::{ToolError, connect, encode_png, parse_chord};

async fn save(address: SocketAddr) -> Result<Vec<u8>, ToolError> {
    let session = connect(address, "ada").await?;
    let window = session.window(None)?.id;
    session.click(window, Point::new(40, 20), Buttons::LEFT, false).await?;
    session.type_text(window, "hello").await?;
    let chord = parse_chord("ctrl+s").expect("a known chord");
    session.keys(window, &[chord]).await?;
    let frame = session.screenshot(window).await?;
    session.close();
    encode_png(&frame)
}
```

## API

- `connect`, `Session`: a tool connection with its windows, the input steps, `resize` and `screenshot`.
- `Session::window`: the window `--window` names by id or exact title, or the first main window.
- `parse_chord`, `known_names`: key names and chords such as `ctrl+s`, as USB HID key codes.
- `encode_png`: a BGRA frame as a PNG file.

## Waiting

Every input step ends with the protocol's `Sync`, so a screenshot taken next sees its result. `type_text` and `keys` also sync before they send, because toolkits drop keys sent in the same instant as the click that focused a field. A host answers a sync after 3 seconds at the latest, so an answer means the input reached the app, not that the app drew. A screenshot the host answers with `Failed` (too many at once) is tried once more.

## License

Apache-2.0
