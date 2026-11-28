---
title: The TUI
description: The views of the devlanes terminal UI and the keys each one takes.
kind: reference
---

`devlanes` or `devlanes ui` opens the TUI on a terminal, and a start on a terminal runs inside it unless you pass `--no-ui`. Closing it leaves the lanes running. `?` lists the keys on screen.

In every list, `j` and `k` or the arrow keys move, `g` and `G` jump to the top and bottom, and `PgUp` and `PgDn` scroll.

On Windows, keys that arrive almost at once are taken as a paste. Pasted text goes only into a text field, such as the ticket row of `n` in the Boxes view. A question such as the one `d` asks takes an answer only after it has been open for a third of a second, and not from a key pressed within a third of a second of the one before.

## Everywhere

| Key | Action |
|---|---|
| `?` | Show the keys |
| `Tab`, `Shift+Tab` | Next or previous project |
| `1` to `9`, `0` | Pick a lane by its place in the bar |
| `M` | Monitor |
| `D` | Daemon view |
| `B` | Boxes view |
| `Esc` | Go back; on the project view, open the menu |
| `U` | Update, when the top bar offers a new release |
| `q`, `Ctrl+C` | Quit; asks whether to stop the running lanes, as [`tui.on_quit`](/docs/reference/config/user/tui/) sets |

## Project view

The list of a lane's units, and the terminal of the selected unit beside it.

| Key | Action |
|---|---|
| `Enter` | Type into the selected unit; `Esc` or `Ctrl+]` stops typing |
| `s` | Start the selected unit, or the lane on the lane row |
| `S` | Start every service of the lane |
| `x` | Stop the selected unit, or the lane |
| `r` | Restart the selected unit; on the lane row, reload or restart the lane |
| `p` | Start the lane with chosen profiles |
| `C` | Clean the lane, after asking |
| `o` | Open the unit's URL in the browser |
| `e` | Show the errors in full |
| `n`, `N` | Next or previous problem |
| `[`, `]` | Previous or next lane |
| `/` | Filter the units by name or `+tag` |
| `z` | Zoom the terminal pane |
| `c` | Select text in the pane; `v` marks, `y` copies |
| `m` | Use a box's lane in place of the one in use here. The popup in [Use the browser extension](../how-to/use-the-browser-extension.md#switch-which-box-serves-a-lane) does the same. |

## Monitor

CPU and memory of every unit.

| Key | Action |
|---|---|
| `<`, `>` | Sort by another column |
| `i` | Reverse the sort |
| `/` | Filter |
| `Enter` | Open the unit in the project view |
| `s`, `x`, `r` | Start, stop or restart the selected unit |

## Daemon view

The daemon's checks and its log.

| Key | Action |
|---|---|
| `R` | Restart the daemon, after asking |
| `S` | Stop the daemon, after asking |
| `X` | Clean the lanes whose worktree is gone, after asking |
| `F` | Pause or follow the log |
| `/` | Filter the log |
| `w` | Change the lowest log level shown |
| `Tab` | Move between the checks and the log |

## Boxes view

The boxes this machine connects to, and the machines connected to this one.

| Key | Action |
|---|---|
| `n` | Connect to a new box |
| `Enter` | Connect, sign in or pair the selected box |
| `d` | Disconnect the selected box |
| `f` | Forget the selected box paired by ticket |
| `p` | Show a pairing ticket and code for another machine |
| `x` | Revoke the selected machine |
