# devlanes-tui-web

The devlanes TUI running in a web page, for the demo on devlanes.run. It compiles the real `devlanes-tui` app and drawing to WebAssembly, runs them on the fake daemon in `devlanes_tui::demo` (the mock data the committed TUI screens use), and draws into an element with Ratzilla's DOM backend. The page's script sends keys and a one-second tick; nothing reaches a real daemon.

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
const demo = new Demo('tui-demo'); // an empty element sized in its font's cells
element.addEventListener('keydown', (event) => {
  if (demo.key(event.key, event.ctrlKey, event.altKey, event.shiftKey)) event.preventDefault();
});
setInterval(() => demo.tick(), 1000);
```

## API

- `Demo` (browser build): `new Demo(id)` mounts into an element, `key(key, ctrl, alt, shift)` returns whether the TUI took the key, `tick()` moves the fake daemon on a second.
- `Session`: the same without the page, for tests and native use.
- `keymap::key_event`: a `KeyboardEvent.key` and its modifiers as the TUI's key event, or `None` for keys the page keeps (Tab, modifiers alone, Alt chords and most Ctrl chords).

## Sizing

The element's CSS size divided by one cell of its font is the TUI's size, fixed when `Demo` is created; the element must be empty, since the backend replaces its content. Ratzilla 0.3's backend reports the window's size rather than the element's, so the terminal's viewport is pinned to the element's grid.

## Releases

`release.yml` builds the bundle on each `v*` tag and attaches `devlanes-tui-web.tar.gz` to the release, outside `release-manifest.json` and `SHA256SUMS`. The website's sync downloads it for the tag it publishes. CI runs clippy for `wasm32-unknown-unknown` on every pull request, so code that only builds natively cannot slip into the crates under the TUI.

## License

Apache-2.0
