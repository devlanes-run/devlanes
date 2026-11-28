# devlanes-gui-viewer

The native GUI viewer: one local window per window of a GUI service, showing the frames its GUI host streams and sending local input back. `devlanes-gui view` runs it; it talks to the host through `devlanes-gui-client` and draws with winit and softbuffer.

## Usage

```rust,no_run
use std::sync::Arc;

use devlanes_gui_codec::RawBackend;
use devlanes_gui_viewer::{Options, ViewerError, run};

fn main() -> Result<(), ViewerError> {
    // On the main thread, which winit needs on macOS and Windows.
    run(Options {
        address: "127.0.0.1:26012".parse().expect("an address"),
        service: "acme@2:desktop".into(),
        decoders: vec![Arc::new(RawBackend)],
        focus: true,
        check: None,
        instance: None,
    })
}
```

## API

- `run`, `Options`, `ViewerError`: run the viewer until the service is gone; `ViewerError::NoDisplay` on a machine without a display.
- `model::Viewer`, `model::Windows`: every decision of the viewer (windows across reconnects, lock states, the close rule, input while locked, when to end), driven by any window system.
- `frame`: the frame a window gets (`decoration`), what its close button does (`close_action`) and the local drag for a move the app starts (`drag`).
- `resize::Resize`: the debounce that sends the final size of a local resize.
- `input`: winit's physical keys as the protocol's HID codes, buttons and wheel deltas.

## Behaviour

A window whose app draws its own frame and reports moves is borderless (on macOS a window with a hidden title bar, which macOS resizes from its edges), and a press on the app's title bar or edge becomes a local OS drag. Other windows keep the OS frame. Their outer close button closes only the local window when the app has a frame inside, until `devlanes gui open` asks again; otherwise it asks the app to close.

A local resize is sent once the size held for 250 ms, and the window shows Resizing from the first change until the first frame after it. While the connection is down every window shows Reconnecting and stays open; a new connection's windows take over the local windows by id, and else in order. The viewer ends 3 seconds after the host closed every window, or after the daemon says the service stopped, which a viewer with a `UnitCheck` asks after 30 seconds without a connection.

Keys go out as physical keys, so the host's layout decides what they type; text an input method commits goes as text. Clipboard text goes to the host when a window gains focus and comes back whenever the app sets it. Held keys and buttons are released when a window loses focus.

On Linux winit and softbuffer load the X11, Wayland and xkbcommon libraries at run time, so the binary links none of them and starts without a display; `run` then fails with `NoDisplay`.

## License

Apache-2.0
