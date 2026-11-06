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
- `html()`: the whole document, its stylesheet and script inline, with this release's version filled in.

## Views

| `view` | Card |
|---|---|
| `lane` | the lane's state as a pill next to how many services run, the start phase and a progress bar while it comes up, one row per service with its URL (failed first, then starting, running and idle); Stop lane and Open, or Restart and Ask agent why when a service failed |
| `board` | one row per lane grouped by project with its branch, state, who started it and URL; the lanes of linked boxes that work here as read-only rows, the others summed up in one line; Manage lanes opens fullscreen with CPU and a stop, start, open and ask button per row |
| `failure` | why a unit failed and the last lines of its log around the error; All lines opens fullscreen with every line, a filter, earlier pages and a line to ask the agent about |
| `clean` | the project's lanes with what a clean deletes and checkboxes; the clean asks to confirm in the action bar |
| `pair` | a pairing code with its countdown and the command to copy |
| `top` | the machine's CPU, memory and pressure in one line, then the running units and containers by CPU with a bar for CPU and, from 520 px, memory, grouped by lane when there are several; Ask agent about the busiest one |

Any other result renders nothing and reports a height of 0, since the host shows its text. Inline cards never scroll and have at most two buttons; a list longer than 15 rows shows the first 15 and sums up the rest in one line whose "Show all" grows the card. Throttled and frozen explain on hover what devlanes did to the idle lane. The fullscreen views appear only where the host lists fullscreen. Every button that changes something also sends `ui/update-model-context`, so the agent knows what the person did; with `structuredContent.read_only` (`devlanes mcp --read-only`) the card shows none of them. Buttons that ask the agent send a plain question with the evidence through `ui/message`, and only once the agent's own call has its result.

Cards poll `app_snapshot` every second while their own lane starts and every 3 seconds otherwise, every 10 seconds while the daemon is not running (they never start it, and keep its last data dimmed), pause while hidden or scrolled out of view and stop after 15 minutes without a click or key press, then show when they were last updated. An empty board does not poll. A result about one lane carries the server's card number for that lane (`structuredContent.card`); when `app_snapshot` reports a higher one in `newest_card`, the older card stops polling and shrinks to one line.

## Styles

The card draws no frame of its own (the server asks for `prefersBorder: true`) and reads only the MCP Apps standard style variables. The bridge writes the host's `styles.variables` onto `<html>`, sets `data-theme` and `color-scheme` from its theme, pads by `safeAreaInsets` and loads `styles.css.fonts` through a constructed stylesheet, which the hash-only `style-src` allows. The card's policy loads fonts from `https://assets.claude.ai` (Claude's Anthropic Sans) and nothing else; other hosts' fonts fall back to the rest of their `--font-sans`. Without host variables the devlanes palette in `src/styles/_tokens.scss` applies under the same names, with the yellow accent on the primary button; a test keeps its shared values equal to `crates/pages`.

## Editing the card

The card's source is a private pnpm package in `ui/`, written in TypeScript with Preact: the views in `src/components/`, the MCP Apps bridge in `src/bridge/`, state and polling in `src/store/`, the data the card reads in `src/types/` (field names as in `crates/proto`) and the SCSS in `src/styles/`. Use Node.js 24.16+ and pnpm 12.6.0, then install dependencies with `pnpm install` in `ui/`.

After any change under `ui/`, run `pnpm build` there and commit `ui/dist/lane.html` with the change. rsbuild writes that single file with the script and stylesheet inline and a Content-Security-Policy that allows only their hashes. `src/lib.rs` embeds it with `include_str!`, so building devlanes never needs Node, and the binary serves whatever `lane.html` was last committed. The `MCP UI` workflow runs `pnpm lint-ci`, `pnpm test-types` and `pnpm build` on every pull request that touches `ui/` and fails when the committed file differs from the build. `pnpm lint-fix` applies Biome's fixes.

The bridge is written by hand instead of using `@modelcontextprotocol/ext-apps`, whose `App` class builds to about 297 kB with zod and the MCP SDK, against about 5 kB for the bridge and an 80 KiB budget for the whole card.

To look at every view, write the previews and serve them (browsers block the card's frame from `file://`):

```sh
cargo test -p devlanes-mcp-ui --test it previews_are_written
python -m http.server 8765 --bind 127.0.0.1 --directory target/mcp-apps-preview
```

Then open `http://127.0.0.1:8765/`. Each page loads the real card in a frame next to a fake host that answers its requests from the fixtures in `tests/it/fixtures.rs` and lists every message both ways. The host, theme and width switch on the page: the fake host sends the style variables Claude, ChatGPT, VS Code, Goose or the ext-apps basic-host send (`tests/it/preview/hosts.js`), or none for the devlanes look, and draws that host's frame around the card.

## Merging the built card

`.gitattributes` marks `ui/dist/lane.html` as generated, hides its minified diff (`-diff`) and sets `merge=ours`, like `Cargo.lock` and the other generated files. With the driver set up once per clone (`git config merge.ours.driver true`, see `CONTRIBUTING.md`), a merge or rebase where both sides changed the file keeps the current side instead of writing conflict markers into minified HTML. That side may be stale, so after any merge or rebase that touched `ui/`, run `pnpm build` in `ui/` and commit the result; CI fails until then, and a test fails if conflict markers got into the file. During a rebase "ours" is the branch you rebase onto.

## License

Apache-2.0. The card bundles Preact (MIT).
