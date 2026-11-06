# devlanes-mcp-ui

The MCP Apps card of `devlanes mcp`: one HTML document that chat hosts (Claude Desktop, VS Code, Cursor, Goose) render next to a `devlanes` tool result. `devlanes-mcp` serves it as a resource; the card picks its view from the result's `structuredContent.view` and reads fresh data through the server's app-only tools `app_snapshot`, `app_logs` and `app_action`.

## Usage

```rust
let resource = (devlanes_mcp_ui::URI, devlanes_mcp_ui::MIME_TYPE, devlanes_mcp_ui::html());
assert_eq!(resource.0, "ui://devlanes/lane.html");
assert!(resource.2.starts_with("<!doctype html>"));
```

## API

- `URI`: `ui://devlanes/lane.html`, for `_meta.ui.resourceUri` and `resources/read`.
- `MIME_TYPE`: `text/html;profile=mcp-app`.
- `html()`: the whole document, its stylesheet and script inline.

## Views

| `view` | Card |
|---|---|
| `lane` | start phases while a lane comes up, then its services with URLs, CPU, restart and stop |
| `board` | every lane with its branch, state, who started it, open, start, stop and ask; boxes read-only |
| `failure` | why a unit failed and its logs: cursors, grep, load earlier, send a line to the agent |
| `clean` | the project's lanes with what a clean deletes, checkboxes and a confirm in the card |
| `pair` | a pairing code with its countdown and copy |

Any other result shows the command and its text. Every button that changes something also sends `ui/update-model-context`, so the agent knows what the person did. Cards poll `app_snapshot` every second while something starts and every 3 seconds otherwise, pause while hidden and stop after 15 minutes without a click or key press.

## Editing the card

Everything is in `ui/`: the askama shell `app.html`, SCSS in `styles/`, and the views in `js/` as plain ES modules with `htm` templates, JSDoc and `// @ts-check` (`ui/jsconfig.json` makes editors check them). There is no Node and no bundler. An inline module script cannot import another inline module, so `build.rs` joins them into one script: the vendored Preact module (`ui/vendor/`) runs inside a function whose exports become constants, then each module in the order `build.rs` lists, with `import` lines, `export` keywords, comments that start a line and indentation removed. All modules share one scope, so two top-level names that collide fail the build. A new module goes into that list after the modules it uses. The build also compiles the SCSS with grass and adds a Content-Security-Policy with the hashes of the inline script and stylesheet.

To look at every view in light and dark, write the previews and serve them (browsers block the card's frame from `file://`):

```sh
cargo test -p devlanes-mcp-ui --test it previews_are_written
python -m http.server 8765 --bind 127.0.0.1 --directory target/mcp-apps-preview
```

Then open `http://127.0.0.1:8765/`. Each page loads the real card in a frame next to a fake host that answers its requests from the fixtures in `tests/it/fixtures.rs` and lists every message both ways.

## License

Apache-2.0. `ui/vendor/standalone.module.js` is htm (Apache-2.0) with Preact (MIT); the icons are Material Design Icons (Apache-2.0).
