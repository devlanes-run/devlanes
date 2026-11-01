# devlanes-tui-web

The devlanes TUI running in a web page, for the demo on devlanes.run: the real `devlanes-tui` app and drawing compiled to WebAssembly, on a fake daemon of its own (`scene`: project `myapp`, four lanes, all healthy, at this release) whose services and containers print new lines every second and whose tasks, starts and stops play out. Everything the demo needs lives in this crate; `devlanes-tui` only builds for the browser. Ratzilla's DOM backend draws it into an element, and the page's script sends keys and a one-second tick; nothing reaches a real daemon.

## Usage

Build the bundle (needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version in `Cargo.lock`; `wasm-opt` is used when present):

```sh
sh packaging/scripts/tui-web.sh out
# out/devlanes-tui-web.tar.gz: devlanes_tui_web.js, devlanes_tui_web_bg.wasm and their .d.ts
```

Then, in the page:

```js
import init, { Demo } from '/tui/devlanes_tui_web.js';

await init();
const element = document.getElementById('tui-demo'); // empty, sized in its font's cells
const demo = new Demo(element.id);
element.addEventListener('keydown', (event) => {
  const { key, ctrlKey, altKey, shiftKey, metaKey } = event;
  if (demo.key(key, ctrlKey, altKey, shiftKey, metaKey)) event.preventDefault();
});
setInterval(() => demo.tick(), 1000);
```

The same session runs natively, which is how the tests drive it:

```rust
let mut session = devlanes_tui_web::Session::new(120, 40);
session.key("M", false, false, true, false); // Shift+M: the monitor
session.tick();
```

## API

- `Demo` (browser build): `new Demo(id)` mounts into an element, `key(key, ctrl, alt, shift, meta)` returns whether the TUI took the key, `tick()` moves the fake daemon on a second, `resize()` measures the element again, `columns` and `rows` give the size in cells, `free()` empties the element.
- `Session`: the same without the page, with `resize(columns, rows)`.
- `keymap::key_event`: a `KeyboardEvent.key` and its modifiers as the TUI's key event, or `None` for keys the page keeps (Tab, modifiers alone, Meta chords, Alt chords other than typed symbols, and most Ctrl chords).
- `scene`: the mock data and output the fake daemon answers with.
- `palette::apply`: the colors the demo gives the TUI's ANSI and default colors.
- `mouse::cell`, `mouse::Wheel`: a point on the page as the cell under it, and wheel deltas as whole scroll steps.

## Mouse

Presses and releases on the element reach the TUI at the cell under the pointer, so rows, tabs and lane chips work with clicks and taps, and a double click types into a unit. The wheel scrolls the TUI only after the demo was clicked and has focus; before that it scrolls the page. Drags and the right button stay with the browser, so text in the demo can be selected and copied like any page text.

## Sizing

The element's CSS size divided by one cell of its font is the TUI's size. The backend replaces the element's content, and a style the demo adds makes its rows and its cell probe take the element's font. On every window resize the demo builds a new grid at the element's size and lays the TUI out again, keeping the view, the selection and an open unit; after changing the element's size or showing it without a window resize, call `resize()`. A hidden element keeps its last grid until it shows. Ratzilla 0.3's backend reports the window's size rather than the element's, so the terminal's viewport is pinned to the grid.

## Releases

`release.yml` builds the bundle on each `v*` tag and attaches `devlanes-tui-web.tar.gz` to the release, outside `release-manifest.json` and `SHA256SUMS`. The website's sync downloads it for the tag it publishes. CI builds the same bundle on every pull request, so code that only builds natively cannot slip into the crates under the TUI.

## License

Apache-2.0
