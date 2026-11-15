---
title: The TUI
description: The views, dialogs and keys of the devlanes terminal UI.
kind: reference
---

The TUI is a client of the daemon, like the CLI. `devlanes` without a command opens it on a terminal, and so does `devlanes ui`. `devlanes start`, `restart`, `lane resume` and `lane new --start` run inside it when they run at a terminal, unless `--no-ui`, `DEVLANES_NO_UI=1` or [`tui.open_on_start`](/docs/reference/config/user/tui/) say otherwise. `?` opens the list of keys on every view.

## Views

| View | Key | Shows |
|---|---|---|
| Project | `Esc` from another view | The top bar with the project tabs and the lane chips, the list of one lane's units, the pane of the selected unit, and a footer with the keys for the selection. |
| Monitor | `M` | A CPU box and a memory box with graphs and a meter per lane, a table of units with their CPU and memory, and graphs for the selected row. |
| Daemon | `D` | A box of daemon facts, a box of checks with a fix for each problem, and the daemon's log. |
| Boxes | `B` | Each box this machine connects to, with its connection and what it mirrors, then this machine as a box and the machines connected to it. |

`M`, `D` and `B` pressed on their own view return to the project view.

### Top bar

The top bar shows one message, the first of these that applies:

| Message | Meaning |
|---|---|
| `daemon stopped · s starts it` | The daemon is not running. |
| `daemon not answering · retrying` | The daemon does not answer. |
| `daemon outdated · U update` | The daemon runs another release than this TUI. |
| `daemon restarting`, `daemon stopping`, `daemon starting` | The daemon is restarting, stopping or starting, with the step under way. |
| `daemon cleaning lanes with missing worktrees…` | A clean of lanes whose worktree is gone runs. |
| `connecting to the daemon…` | The TUI has no connection yet. |
| An error | The last action failed. A click opens the error list. |
| A start's progress | The start this TUI was opened for, then its result. |
| A notice | The answer to the last key, such as `daemon already running`. |
| `starting BRANCH: ...` | A lane of the project is starting, with its step or the number of ready services. |
| A release | A newer release is available, installing, failed or ready. `U` or a click acts on it. |
| `N failed`, `N orphaned` | Units of the project in these states. A click selects the first one. |

Below the bar, one tab per project when the daemon knows several, and one chip per lane of the project, numbered 1 to 9, then 0. A grey lane chip marks a lane that claims the same hostnames and ports as a box's lane or a local lane of the project, and is not the one this machine uses.

### Glyphs

The key list opens with the glyphs the list draws. [`tui.glyphs`](/docs/reference/config/user/tui/) set to `ascii` draws them in ASCII.

| Glyph meaning | Unit state |
|---|---|
| ready (check passed) | `ready` |
| running (no ready check) | `ready` with a ready check that is only that the process started |
| throttled | `ready`, its lane throttled |
| frozen | `ready`, its lane frozen |
| stopped, not run | `stopped`, `idle` |
| starting, stopping (a spinner) | `starting`, `stopping` |
| queued | `queued` |
| failed | `failed` |
| orphaned | `orphaned` |
| task done | `exited` |
| the unit's color | A bar in the color of the unit |
| not in use here | A lane chip this machine does not use |

[States and exit codes](states-and-exit-codes.md) describes each state.

### Daemon unavailable screen

While the daemon is stopped, restarting or unreachable, or runs another release, every view shows this screen in place of its body: the steps of a stop or restart under way, then the daemon's live log. It is not a view of its own and closes when the daemon is back.

## Keys

Keys are case-sensitive: `S` is Shift and `s`. A click on a key in the footer presses it. The keys of each view work while no dialog is open; an open dialog takes every key.

### Everywhere

| Key | Action |
|---|---|
| `?` | Open the key list. Any key closes it. |
| `Tab`, `Shift+Tab` | Next or previous project. On the daemon view, move between the checks and the log. |
| `Shift+1` to `Shift+9` | Pick a project by its place in the tabs. |
| `1` to `9`, `0` | Pick a lane by its place in the chips, and return to the project view. |
| `M` | Open the monitor, or return from it. |
| `D` | Open the daemon view, or return from it. |
| `B` | Open the boxes view, or return from it. |
| `Esc` | One step back. On the project view with nothing to cancel, open the menu. |
| `U` | Act on the release the top bar offers. A newer release: update to it, or restart into it once it is installed, after asking. A daemon newer than this TUI: close and open the TUI installed now. |
| `q`, `Ctrl+C` | Quit. While lanes of the project on screen run, do what [`tui.on_quit`](/docs/reference/config/user/tui/) says, which by default opens the quit dialog. Pressed again while lanes stop, quit without waiting. |

A box's lanes do not count as running for `q`: the box stops them, not this machine.

### Project view, list

| Key | Action |
|---|---|
| `j`, `Down` | Move down. |
| `k`, `Up` | Move up. From the top row, select the lane. |
| `g`, `Home` | Select the lane row. |
| `G`, `End` | Select the last row. |
| `t` | Select the tasks section. |
| `i` | Select the infra section. |
| `h`, `Left` | Fold the row, or move to its section. |
| `l`, `Right` | Unfold the row. |
| `Enter`, `Space` | Fold or unfold a section, or type into the selected unit when it runs. |
| `[`, `]` | Previous or next lane of the project. |
| `n`, `N` | Next or previous problem: a failed or orphaned unit, or a failed action. |
| `e` | Open the error list on the newest error. |
| `s` | Start the selected unit, or run the selected task. On the lane row, start the lane with its default services. |
| `S` | Start every service of the lane that is not running. |
| `x` | Stop the selected unit, or the lane on the lane row. |
| `r` | Restart the selected unit. On the lane row, reload the lane when its project file changed, else restart it. |
| `p` | Open the profile picker to start the lane with profiles. |
| `C` | Clean the lane. Asks first. |
| `o` | Open the selected unit's URL in a browser. |
| `m` | Use the selected grey lane instead of the lane in use. |
| `/` | Filter the list by name, or by tag with `+tag`. |
| `z` | Zoom the pane to the full screen, or back. |
| `c` | Enter copy mode in the pane. |
| `PgUp`, `PgDn` | Scroll the pane. Older lines come from the stored logs. |

While the filter takes typing:

| Key | Action |
|---|---|
| Text, `Backspace` | Change the filter. |
| `Up`, `Down` | Move through the matching rows. |
| `Enter` | Keep the filter and leave it. An empty filter is removed. |
| `Esc` | Remove the filter. |

On a box's lane, `s`, `S`, `x`, `r`, `p`, `C`, `c`, `PgUp` and `PgDn` do nothing and the top bar names the command to run on the box. The pane of a box's unit shows its details in place of a terminal: its state, its ports with their mirror state, its hostnames with the URL that opens on this machine, and its synced folders.

### Project view, pane

Typing into a unit sends every key to its terminal except the keys that end typing.

| Key | Action |
|---|---|
| `Esc` | Stop typing. In a program that uses the full screen, such as an editor, `Esc` goes to the program. |
| `Ctrl+]` | Stop typing, in every program. |
| `Esc` in the list | With the pane scrolled back, return it to the live screen. |

Copy mode, entered with `c`:

| Key | Action |
|---|---|
| `h`, `j`, `k`, `l`, arrows | Move the cursor. At the top row, `k` scrolls back. |
| `0`, `Home` | Start of the row. |
| `$`, `End` | End of the row. |
| `v`, `Space` | Start or drop a selection at the cursor. |
| `y`, `Enter` | Copy the selection, or the cursor's row without one, and leave copy mode. |
| `Esc`, `q` | Leave copy mode. |

Copying sends the text to the terminal with OSC 52. Some terminals block it.

### Monitor

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Move the selection. |
| `PgDn`, `PgUp` | Move the selection by half a screen. |
| `g`, `Home`, `G`, `End` | First or last row. |
| `Left`, `<`, `Right`, `>` | Sort by the previous or next column. |
| `i` | Reverse the sort. |
| `e` | Group the units by lane, or list them flat. |
| `/` | Filter the table. `Enter` keeps the filter, `Esc` removes it. |
| `h` | Fold the lane, or move to the lane of the selected unit. |
| `l` | Unfold the lane. |
| `Enter`, `Space` | On a lane, fold or unfold it. On a unit, open it in the project view. |
| `z` | Let the graphs of the selected row fill the page, or back. |
| `c`, `m` | Show CPU or memory in the large box. |
| `s`, `x`, `r` | Start, stop or restart the selected unit or lane. |
| `Esc` | Leave the zoom, then remove the filter, then return to the project view. |

### Daemon view

| Key | Action |
|---|---|
| `R` | Restart the daemon. Asks first. |
| `S` | Stop the daemon. Asks first. |
| `s` | Start the daemon when it is stopped. |
| `X` | Clean every lane whose worktree is gone. Asks first. |
| `j`, `Down`, `k`, `Up` | Scroll the log by one entry. Scrolling up pauses it. |
| `PgDn`, `PgUp` | Scroll the log by half a screen. |
| `g`, `Home` | Oldest entry. |
| `G`, `End` | Newest entry, and follow the log. |
| `F` | Pause or follow the log. |
| `d` | Fold or unfold repeated entries. |
| `w` | Show entries from the next level up. |
| `/` | Filter the log by text. `Enter` keeps the filter, `Esc` removes it. |
| `T` | Open the target picker. |
| `i` | Show every check, or problems only. |
| `c` | Enter copy mode in the log, or in the checks box when it has the focus. |
| `Esc` | Remove the text filter, then return to the project view. |

With the checks box focused by `Tab`, `j`, `k`, `PgUp`, `PgDn`, `g` and `G` scroll the checks instead of the log.

Copy mode in the log and the checks box has the keys of copy mode in the pane, and these:

| Key | Action |
|---|---|
| `g`, `G` | First or last line. |
| `PgUp`, `PgDn` | Move by half a screen. |
| `Y` | Copy the whole entries the selection touches, as `daemon.log` has them. |
| `c` | Leave copy mode. |

On the daemon unavailable screen, `q` and `Ctrl+C` quit without asking, `s` starts a daemon that is stopped or does not answer, `U` updates and `R` restarts a daemon of another release, and the log keys above work.

### Boxes view

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Move through the boxes, then the connected machines. |
| `PgDn`, `PgUp`, `g`, `Home`, `G`, `End` | Move by a page, or to the first or last row. |
| `Enter` | On a box, connect, sign in, enter a pairing code or disconnect, by its state. On a grey lane, use it. |
| `m` | Use the selected grey lane. |
| `n` | Open the connect dialog for a new box. |
| `p` | Show a pairing code for another machine. |
| `o` | Open the options at [`remote.allow_connections`](/docs/reference/config/user/remote/). |
| `d` | Disconnect the selected box. Asks first. |
| `x` | Revoke the selected connected machine. Asks first. |

### Mouse

| Action | Effect |
|---|---|
| Click a project tab, lane chip or footer key | Select it, or press the key. |
| Click a list row | Select it. On a section's arrow, or on a selected section, fold or unfold it. |
| Double-click a unit row | As `Enter`. |
| Click the pane of a running unit | Type into it. |
| Drag in the pane | Select text and copy it on release. |
| Wheel | Scroll the list, the pane, the monitor table, the log or the error list, or move the selection on the boxes view. |
| Click outside a dialog | Close it, as `Esc`. A dialog that asks a question or saves ignores the click. |

A program that reads the mouse gets the clicks in its pane while you type into it.

## Dialogs

| Dialog | Opens with | Holds |
|---|---|---|
| Welcome | The first TUI in a devlanes home | What devlanes does, the first keys and the docs link. |
| Release highlights | The first TUI after an update, and the menu | The breaking changes, then the highlights, of each release since the one seen before. |
| Menu | `Esc` on the project view | The banner, the version and the items options, highlights, help and quit. |
| Options | `o` in the menu | Every user config setting in tabs, with its value and description. |
| Key list | `?`, `h` in the menu | The glyphs, then every key, the current view first. |
| Quit | `q` while lanes run | The lanes that run, the choices, and a box to remember the choice. |
| Confirm | A key that stops, cleans, disconnects, revokes or updates; a task that asks before it runs; a machine without HTTPS and hostnames set up | The question and what it does. |
| Profile picker | `p` | The project's profiles with their default and the services they include. |
| Error list | `e`, a click on the error in the top bar | Each error this TUI received, in full, with the failed command and the hint. |
| Target picker | `T` | The targets in the daemon's log, with a count each. |
| Connect | `n`, `Enter` on a box | The form for a new box, then the connection's progress and its questions: a passphrase, a host key, a pairing code. |
| Pairing code | `p` | A code for another machine to pair with this one. |

### Welcome

| Key | Action |
|---|---|
| `Enter`, `Esc`, `Space` | Close it. It does not open again in this devlanes home. |
| `o` | Open the docs in a browser. |
| `q`, `Ctrl+C` | Quit. |

### Release highlights

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Scroll a line. |
| `PgDn`, `Space`, `PgUp` | Scroll ten lines. |
| `g`, `Home`, `G`, `End` | Top or bottom. |
| `o` | Open the release page in a browser. |
| `Enter`, `Esc` | Close it. |
| `q`, `Ctrl+C` | Quit. |

### Menu

| Key | Action |
|---|---|
| `j`, `Down`, `Tab`, `k`, `Up`, `Shift+Tab` | Move. |
| `g`, `Home`, `G`, `End` | First or last item. |
| `Enter`, `Space` | Pick the item. |
| `o`, `l`, `h`, `q` | Pick options, highlights, help or quit. |
| `Esc` | Close the menu. |
| `Ctrl+C` | Quit. |

### Options

The options are in tabs: general, network, startup and storage. Changes stay on the page until you save them to the user config, which keeps its comments and order.

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Move. |
| `PgDn`, `PgUp`, `g`, `Home`, `G`, `End` | Move by a page, or to the first or last setting. |
| `l`, `Right`, `Tab`, `h`, `Left`, `Shift+Tab` | Next or previous tab. |
| `1` to `4` | Pick a tab. |
| `Enter`, `Space` | Edit the setting. |
| `d`, `Delete`, `Backspace` | Put back the default. |
| `s`, `Ctrl+S` | Save every change. |
| `R` | Restart the daemon, after saving a setting that needs it. |
| `Esc`, `q` | Return to the menu. With unsaved changes, ask first. |
| `Ctrl+C` | Quit. With unsaved changes, ask first. |

While a setting is edited:

| Key | Action |
|---|---|
| `Left`, `Right` | Change the value, or step a number. |
| `h`, `l`, `Space` | Change a value that is not typed. |
| Text, `Backspace`, `Ctrl+U` | Type, delete a character, or clear a typed value. |
| `d`, `Delete` | Put back the default. `Delete` only, in a typed text value. |
| `Enter` | Keep the value. |
| `Up`, `Down` | Keep the value and move. |
| `Esc` | Put back the value from before the edit. |

The unsaved-changes question takes `s` or `y` to save, `d` or `n` to discard, and `Esc`, `k` or `q` to keep editing; `Left`, `Right`, `Tab` and `Enter` pick an answer.

### Quit

| Key | Action |
|---|---|
| `j`, `Down`, `Tab`, `k`, `Up`, `Shift+Tab` | Pick a choice: close and keep the lanes running, close and stop them, or cancel. |
| `Space`, `r` | Tick or untick "remember this choice", which writes [`tui.on_quit`](/docs/reference/config/user/tui/). |
| `Enter` | Act on the choice. |
| `q` | Close and keep the lanes running. |
| `Esc`, `n` | Cancel. |
| `Ctrl+C` | Quit at once. |

### Confirm

| Key | Action |
|---|---|
| `y` | Do it. |
| `Enter` | Do it. Questions that stop or restart the daemon, clean lanes whose worktree is gone, stop a local lane to use a box's lane, disconnect a box, revoke a machine or restart into a release take `y` only. |
| `n`, `q`, `Esc` | Cancel. |

### Profile picker

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Move. |
| `Space` | Tick or untick the profile. |
| `Enter` | Start the lane with the ticked profiles. With none ticked, start it with the default profile, or without a profile when the project has no default. |
| `Esc`, `q` | Close it. |

### Error list

| Key | Action |
|---|---|
| `h`, `Left`, `l`, `Right` | Older or newer error. |
| `j`, `Down`, `k`, `Up` | Scroll a line. |
| `PgDn`, `PgUp` | Scroll ten lines. |
| `y` | Copy the error. |
| `Esc`, `Enter`, `q`, `e` | Close it. |

### Target picker

| Key | Action |
|---|---|
| `j`, `Down`, `k`, `Up` | Move. |
| `Space` | Tick or untick the target. |
| `a` | Untick every target, which shows them all. |
| `Enter` | Show the log of the ticked targets. |
| `Esc`, `q` | Close it and keep the filter as it was. |

### Connect

On the form, the first row chooses how the box is reached: an SSH destination, or a command that runs `devlanes tunnel --stdio` in the box, which is saved under [`remote.boxes.<name>.command`](/docs/reference/config/user/remote/).

| Key | Action |
|---|---|
| `Tab`, `Down`, `Shift+Tab`, `Up` | Move between the rows. |
| `Left`, `Right`, `Space` on the first row | Switch between SSH and a command. |
| Text, `Backspace`, `Ctrl+U` | Type, delete a character, or clear the field. |
| `Enter` | Connect. |
| `Esc` | Close the dialog. A connection under way goes on in the daemon. |

While the connection asks a question, text, `Backspace` and `Ctrl+U` edit the answer and `Enter` sends it. After a failed connection, `Enter` tries again and `q` closes the dialog.

### Pairing code

| Key | Action |
|---|---|
| `p` | Get a new code. |
| `y` | Copy the code. |
| `o` | Open the options at [`remote.allow_connections`](/docs/reference/config/user/remote/), while connections to this machine are off. |
| `Esc`, `Enter`, `q` | Close it. |

## When the TUI closes

Closing the TUI leaves the lanes running unless you chose to stop them. The terminal then shows the lane that was on screen, whether the daemon still runs, a newer release with the command that installs it, and the command that stops the lane while it runs.
