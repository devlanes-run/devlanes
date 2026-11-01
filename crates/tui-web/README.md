# devlanes-tui-web

The devlanes TUI running in a web page, for the demo on devlanes.run: the real `devlanes-tui` app and drawing compiled to WebAssembly, on the fake daemon in `devlanes_tui::demo` (the mock data the committed TUI screens use). Ratzilla's DOM backend draws it into an element, and the page's script sends keys and a one-second tick; nothing reaches a real daemon.

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
  // Command shortcuts belong to the browser; `key` has no Meta flag.
  if (event.metaKey) return;
  if (demo.key(event.key, event.ctrlKey, event.altKey, event.shiftKey)) event.preventDefault();
});
setInterval(() => demo.tick(), 1000);
```

The same session runs natively, which is how the tests drive it:

```rust
let mut session = devlanes_tui_web::Session::new(120, 40);
session.key("M", false, false, true); // the monitor
session.tick();
```

## API

- `Demo` (browser build): `new Demo(id)` mounts into an element, `key(key, ctrl, alt, shift)` returns whether the TUI took the key, `tick()` moves the fake daemon on a second, `free()` empties the element.
- `Session`: the same without the page.
- `keymap::key_event`: a `KeyboardEvent.key` and its modifiers as the TUI's key event, or `None` for keys the page keeps (Tab, modifiers alone, Alt chords other than typed symbols, and most Ctrl chords).
- `palette::apply`: the colors the demo gives the TUI's ANSI and default colors.

## Sizing

The element's CSS size divided by one cell of its font is the TUI's size, fixed when `Demo` is created. The element must be empty, since the backend replaces its content, and must keep its size: Ratzilla rebuilds its grid on every window resize, and a grid of another size makes the next call throw. Ratzilla 0.3's backend reports the window's size rather than the element's, so the terminal's viewport is pinned to the element's grid.

## Releases

`release.yml` builds the bundle on each `v*` tag and attaches `devlanes-tui-web.tar.gz` to the release, outside `release-manifest.json` and `SHA256SUMS`. The website's sync downloads it for the tag it publishes. CI builds the same bundle on every pull request, so code that only builds natively cannot slip into the crates under the TUI.

## License

Apache-2.0
