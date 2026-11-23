---
title: Use the browser extension
description: See your lanes and their URLs in Chrome, Chromium, Brave or Edge, open a page in another lane, and pick which box serves a lane.
kind: how-to
publish: false
---

The devlanes browser extension lists your projects and lanes with their URLs in a popup, and the toolbar badge shows the lane number of the current tab. It works in Chrome, Chromium, Brave and Edge.

## Build and load the extension

The extension is built from its repository, devlanes-run/browser. You need Node.js 24.16 or later and pnpm 12.6.

```console
$ git clone https://github.com/devlanes-run/browser
$ cd browser
$ pnpm install
$ pnpm build
```

Open `chrome://extensions`, or `edge://extensions` in Edge, and turn on Developer mode. Click "Load unpacked" and pick the `apps/chromium/dist` folder.

## Register devlanes with the browser

```console
$ devlanes setup
```

Setup registers devlanes as the extension's native host. Reopen the popup afterwards. The browser doesn't need a restart.

## Open the popup

Click the devlanes icon in the toolbar. The popup shows whether the daemon runs, then each project with its lanes, their branch, state and routes. The lane of the current tab is highlighted.

The extension doesn't keep the daemon running, so the daemon can stop while the popup is open, as the TUI's exit summary says. Run `devlanes` in a terminal to start it again.

## Open this page in another lane

On a lane's page, pick another lane of the project under "Open this page in". The extension opens the same service on that lane and keeps the path, query and hash.

## Switch which box serves a lane

When a box runs the same lane as your machine, the lane offers a box picker. Pick the box, or this machine. [Work with several boxes](remote/several-boxes.md) covers what that changes. When the lane runs on your machine, the popup asks before it stops it and switches.

## Related

- [Install devlanes](../install.md)
- [Work with several boxes](remote/several-boxes.md)
- [Limits](../reference/limits.md)
