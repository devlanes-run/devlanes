# devlanes-tui

The terminal UI opened by bare `devlanes`, `devlanes ui` and `devlanes start` on a terminal. It is a daemon client like the CLI: it talks to the daemon through `devlanes-client` and shows each project's lanes with a live terminal for the selected unit, a resource monitor, a daemon page, and the lanes of connected boxes, read-only. Its boxes view (`B`) connects, signs in to, pairs with and disconnects boxes, shows a pairing code for another machine, and lists and revokes the clients of this machine as a box.

## Usage

```rust
use devlanes_tui::{Options, Paths};

#[tokio::main]
async fn main() -> Result<(), devlanes_tui::Error> {
    let options = Options {
        // No tab progress bar; theme and glyphs detected from the terminal.
        progress: None,
        theme: None,
        glyphs: None,
    };
    devlanes_tui::run_with(Paths::discover()?, options).await
}
```

`run_with` connects first, spawning the daemon if needed, and returns `Error::NotATerminal` without a TTY. `run(paths)` is the same with default options.

`run_start` opens the TUI on a start that is already running, for `devlanes start` on a terminal. The binary sends the start request with `events: true`, reads its answer with `start::follow_start`, and waits with `start::await_start` until the daemon accepted it (a refusal comes back as an error for the CLI to print). The TUI then shows the started lane, the running phase in the top bar and, when the result arrives, the result line. `run_start` returns `StartExit::Ready`, `Failed` or `Pending` (quit before the result; the daemon keeps starting).

`?` in the TUI lists every key, the current screen's first, with a legend of the state glyphs.

## API

- `run`, `run_with`, `run_start` and `Options` are the entry points for the binary.
- `DaemonApi` is every daemon call the TUI makes; `ClientApi` implements it over `devlanes-client`, tests use a fake.
- `drive` runs the event loop on any ratatui backend with any event stream.
- `app::App::update` turns messages into effects without I/O; `ui::render` draws an `App`.
- `tree` builds a lane's rows; `monitor` sorts, filters and groups the monitor's rows; `graph` draws braille graphs, sparklines and meters in the load gradient.
- `boxes` reads `host.list` and turns each connected box's registry into read-only lanes (`acme@1 @hetzner` in project `acme`), each a `BoxLane` that names its box; `clients` reads `remote.clients`.
- `connect` holds the boxes view's modals: the new connection form, the prompts of an attached `connect` and the pairing code.

## Config example

```yaml
# ~/.devlanes/config.yaml
tui:
  theme: auto    # auto, dark, light or ansi
  glyphs: auto   # auto, unicode or ascii
  on_quit: ask   # ask, keep or stop: q while lanes run
terminal:
  progress: auto # start progress in the tab's progress bar
```

## Behaviour

Quitting closes connections; units, tasks and containers keep running in the daemon. While lanes of the project on screen run, `q` asks whether to keep them or stop them first, unless `tui.on_quit` (`ask`, `keep` or `stop`) says; the dialog's "remember" box writes that setting.

Once the TUI closed, `run_with` and `run_start` print the devlanes banner, the lane shown last with its state, whether the daemon still runs and how many lanes it runs, and `devlanes stop --lane <lane>` while that lane runs (`exit_summary`). The daemon gets one second to answer `status` for it.

Moving through the list watches a unit without resizing its PTY. Enter, a double click or a click in the pane types into it: the stream becomes the PTY's owner where it is (`attach.role`) and the pane takes over its size, as zoom does. Esc or Ctrl+] ends typing; inside full-screen programs only Ctrl+], which crossterm may report as Ctrl+5. Units on pipes (`pty: false` from the daemon) are never resized.

A unit's pane is one surface: the lines that left the unit's screen above the screen itself, both from the daemon's `attach.view` events, and above those the unit's log, read a page at a time as scrolling nears the top of what the pane keeps. Scrolling moves over all of it alike.

Connecting a box from the boxes view sends `connect` on a connection of its own and stays attached while the link settles, so the daemon relays ssh's prompts and the box's pairing code to the TUI, as it does to `remote connect`. Answers go back as binary frames on that connection. Esc drops it, and the daemon keeps the connection to the box. A box reached by a command is first saved as `remote.boxes.<name>.command` in `config.yaml`, with the same edit `devlanes config set` makes.

A box lane joins the tab of the project with its name, and a project only boxes run gets one tab. In a tab with box lanes the chips go by number, this machine's lane first, then the boxes by name: `@0  @0 fork-a  @0 fork-b  @1`. When lanes clash on hostnames, the daemon mirrors one of them, and the others are grey: dim, with a hollow dot, or in brackets with ascii glyphs. `m` on a grey lane sends `remote.use` to make it the one in use. While this machine's lane runs, `m` asks first, stops that lane and switches once it stopped. In the boxes view, `m` works on a box's skipped lane row too.

While lanes start, the terminal tab's progress bar shows ready services over the services starting, across every starting lane. A failed start shows red for three seconds. Quitting, a panic or `terminal.progress: never` clears it; see `devlanes-term` for where it is drawn.

## Screens for design review

`screens/` holds every page and dialog drawn against a fake 45-unit monorepo at 200x60, 120x40 and 80x24, one `.txt` per state with the screen and the styles of its cells, and `index.md` listing them. They are committed so a pull request shows what changed on screen. The `screens_are_current` test fails when they are stale, and `DEVLANES_BLESS=1 cargo nextest run -p devlanes-tui -E 'test(screens_are_current)'` rewrites them; `screens/AGENTS.md` has the details.

`cargo run -p devlanes-tui --example screens` writes the same files plus a PNG and an ANSI dump of each state, which are gitignored. The PNGs use Menlo or DejaVu Sans Mono; `DEVLANES_SCREENS_FONT` names another font file.

Only the selected unit is parsed client-side. tui-term 0.3 depends on vt100 0.16, so it renders through a newtype over vt100 0.18 (`screen::ScreenView`).

## License

Apache-2.0
