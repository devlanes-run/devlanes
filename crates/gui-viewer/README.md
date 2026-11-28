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
- `input`: winit's physical keys as the protocol's HID codes, buttons and wheel deltas; `model::Viewer::key` decides between text and physical keys.

## Behaviour

A window whose app draws its own frame and reports moves is borderless (on macOS a window with a hidden title bar, which macOS resizes from its edges), and a press on the app's title bar or edge becomes a local OS drag. Other windows keep the OS frame. Their outer close button closes only the local window when the app has a frame inside, until `devlanes gui open` asks again; otherwise it asks the app to close.

A local resize is sent once the size held for 250 ms, and the window shows Resizing from the first change until the first frame after it. The first size the OS gives a window within 500 ms after the viewer set one is taken as it is, and a size the app answered with another one is not sent again until the user settles on a different size. While the connection is down every window shows Reconnecting and stays open; a connection that sends no bytes for 16 seconds is dropped and dialed again. A new connection's windows take over the local windows by id, or in order when no id matches (a new host). The viewer ends 3 seconds after the host showed every window Closed, or once the daemon says the service stopped: with a `UnitCheck` it asks after 30 seconds without a connection, without starting a daemon, and a daemon that stays away for 30 seconds more counts as the service gone.

A key that types a character with the local layout goes out as that text, so any layout types what is on its keys; dead keys and AltGr work the same way, and the host inserts the text as given whatever modifiers are held. Shortcuts go out as physical keys: with Control, with Super or Command, and on Windows and Linux with Alt alone (menu mnemonics; on macOS Option types characters). Space and the keys that type nothing (arrows, Enter, modifiers) go out as physical keys too; a keypad key that types nothing goes as the navigation key it means, such as Left for keypad 4 with Num Lock off. A key keeps the way its press went out until its release. Caps Lock and Num Lock stay on this machine. While a lane is frozen only clicks go out, to wake it. Clipboard text goes to the host when a window gains focus, unless the app's clipboard text arrived while it was read, and comes back whenever the app sets it. Held keys and buttons are released when focus leaves every window of the service; moving between them is decided at the end of that turn of the event loop and is no focus loss for the app. Frames wait in one slot per window, so a slow window skips frames instead of queueing them.

Limits: no input method is used, so there is no preedit text while composing, and an app sees a held printable key as repeated text, not as a key held down. On Windows a real Control+Alt+letter shortcut goes out as text whenever the layout types a character for it, since Windows reports AltGr as Control and Alt. The Caps Lock key itself never reaches the app, so an app or a key remap on the host that reacts to that key does not see it.

On Linux winit and softbuffer load the X11, Wayland and xkbcommon libraries at run time, so the binary links none of them and starts without a display; `run` then fails with `NoDisplay`.

## License

Apache-2.0
