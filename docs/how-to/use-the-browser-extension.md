---
title: Use the browser extension
description: See your lanes and their URLs in Chromium-based browsers such as Chrome, Edge and Brave, open a page in another lane, and pick which box serves a lane.
kind: how-to
---

The devlanes browser extension lists your projects and lanes with their URLs in a popup, and the toolbar badge shows the lane number of the current tab. It works in Chromium-based browsers such as Chrome, Edge and Brave. [Fill test logins on lanes](fill-test-logins.md) covers filling the sign-in forms of a project's lanes.

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

Until devlanes is registered, the popup says "Not set up" and asks you to run setup.

```console
$ devlanes setup
```

Setup registers devlanes as the extension's native messaging host in each browser it registers, the way Claude Code registers itself for [Claude in Chrome](https://code.claude.com/docs/en/chrome). [Install devlanes](../install.md) lists those browsers. Reopen the popup afterwards. The browser doesn't need a restart.

## Open the popup

Click the devlanes icon in the toolbar. The popup shows whether the daemon runs, then each project with its lanes, their branch, state and routes. Click a route to open it. A wildcard route, such as `*.myapp.localhost`, has no single page, so it shows as text. The lane of the current tab is marked "This tab".

On a lane's page, the badge shows the lane number. It has a dot and turns blue when a box serves the lane.

The extension doesn't keep the daemon running, so the daemon can stop while the popup is open, as the TUI's exit summary says. Run `devlanes` in a terminal to start it again.

## Open this page in another lane

On a lane's page, open the popup and pick another lane of the project under "Open this page in". The extension opens the same service on that lane and keeps the path, query and hash. On a wildcard hostname, such as a tenant page, it keeps the tenant. When the other lane has no route for the service, it opens that lane's first route.

## Switch which box serves a lane

When more than one machine has a lane, such as your machine and a box, the lane shows a "Served by" row with "This machine" and each box. Pick the one that should serve the lane. [Work with several boxes](remote/several-boxes.md) covers what that changes.

When the lane runs on your machine, the popup asks before it stops it. "Stop and switch" stops the lane and then switches. When the lane's containers hold its ports, the popup lists the commands that stop them, each with a copy button. Run them, then click "Switch again".

## If setup doesn't register your browser

Setup keeps a copy of the extension's host file, its manifest, for any other Chromium-based browser. Point the browser at the copy by hand.

::::tabs{sync="os"}
:::tab[macOS]
The copy is `~/.devlanes/browser/run.devlanes.browser.json`. The browser's `chrome://version` page shows its Profile Path. Its user data folder is that path without the profile folder at the end, such as Default. Link the copy into that folder's NativeMessagingHosts folder.

```console
$ mkdir -p "<user data folder>/NativeMessagingHosts"
$ ln -s ~/.devlanes/browser/run.devlanes.browser.json "<user data folder>/NativeMessagingHosts/"
```
:::
:::tab[Linux]
The copy is `~/.devlanes/browser/run.devlanes.browser.json`. The browser's `chrome://version` page shows its Profile Path. Its user data folder is that path without the profile folder at the end, such as Default. Link the copy into that folder's NativeMessagingHosts folder.

```console
$ mkdir -p "<user data folder>/NativeMessagingHosts"
$ ln -s ~/.devlanes/browser/run.devlanes.browser.json "<user data folder>/NativeMessagingHosts/"
```
:::
:::tab[Windows]
The copy is `%USERPROFILE%\.devlanes\browser\run.devlanes.browser.json`. Most Chromium-based browsers read Chromium's key and then Chrome's, so a browser may already find devlanes when setup registered Chrome, Chromium or Brave. Otherwise point Chromium's key at the copy. A browser whose documentation names a key of its own reads that key first.

```console
PS> reg add "HKCU\Software\Chromium\NativeMessagingHosts\run.devlanes.browser" /ve /t REG_SZ /d "$env:USERPROFILE\.devlanes\browser\run.devlanes.browser.json" /f
```
:::
::::

Each run of setup keeps the copy current, so the link or key keeps working. `devlanes setup --remove` deletes the copy and any key under Chromium's or Chrome's location that points at it. Delete any other link or key you made yourself.

## Related

- [Install devlanes](../install.md)
- [Fill test logins on lanes](fill-test-logins.md)
- [Work with several boxes](remote/several-boxes.md)
- [Limits](../reference/limits.md)
