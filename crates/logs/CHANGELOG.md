# devlanes-logs

## 0.13.0 — 2026-11-09

### Patch changes

- [c69d9e36](https://github.com/devlanes-run/devlanes/commit/c69d9e36d60f3d73326f2d1a7d0fac3214461188) Fixed `devlanes logs -f --grep` and `--level` showing lines that arrive while following without filtering them.
  
  Lines that arrive while following also keep their colors and get the `--context` lines around matches, as the first page does.

## 0.7.1 ... 0.12.0 — 2026-11-07

Version bump only, to follow other packages.

## 0.7.0 — 2026-10-28

### Patch changes

- [0e02c1b](https://github.com/devlanes-run/devlanes/commit/0e02c1b8e218bb470869e1e98c0631a5586ebf85) Changed how devlanes keeps a unit's output: `devlanes logs` holds each line once, as the unit's terminal showed it, also after a Windows console repaints its window on a resize, and the TUI's unit pane scrolls from the screen into older output and on into the log as one view, without flickering or switching to a separate stored-logs view.
  
  Typing into a unit or zooming its pane keeps its output on screen instead of reopening the stream. Lines a program rewrites in place (progress lines) are logged again with their new text, and output a program draws on the alternate screen (editors, pagers) is not logged.

## 0.4.0 ... 0.6.0 — 2026-10-26

Version bump only, to follow other packages.

