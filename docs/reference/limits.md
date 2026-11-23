---
title: Limits
description: What devlanes does not do, each with the issue that tracks it.
kind: reference
---

What devlanes does not do, with the issue that tracks each limit. An empty Issue cell means no issue tracks it.

## Projects and services

| Limit | Issue |
|---|---|
| A project name belongs to one repository; another repository cannot use it. | |
| A variable cannot be unset. | |
| A service has no `env_file`; `.env` values reach it only through `${{ env.NAME }}`. | [#186](https://github.com/devlanes-run/devlanes/issues/186) |
| The user config has no values or secrets shared by every project. | [#137](https://github.com/devlanes-run/devlanes/issues/137) |
| A service has no built-in debug port. | [#139](https://github.com/devlanes-run/devlanes/issues/139) |
| The services of a lane start at the same time; a start cannot bring them up one after another. | [#197](https://github.com/devlanes-run/devlanes/issues/197) |
| Resources outside Docker, such as a hosted database branch, cannot be created per lane. | [#59](https://github.com/devlanes-run/devlanes/issues/59) |
| `devlanes run` passes no extra arguments to a task's command. | |
| `devlanes wait` exits with 1 for a command that failed, not with the command's own exit code. | |
| The daemon has no public API for other programs. | [#29](https://github.com/devlanes-run/devlanes/issues/29) |

## Hostnames, HTTPS and the proxy

| Limit | Issue |
|---|---|
| A lane's bare domain, such as `myapp1.localhost`, gets no page when no service routes it. | [#100](https://github.com/devlanes-run/devlanes/issues/100) |
| gRPC over HTTP/2 does not go through a hostname. | |
| No public DNS name points at a lane. | |
| The CA is not installed in iOS simulators or Android emulators. | |
| Cookies, storage and saved passwords cannot be shared between lanes. | [#190](https://github.com/devlanes-run/devlanes/issues/190) |

## Browser extension

| Limit | Issue |
|---|---|
| The extension runs in Chromium-based browsers, not in Firefox. | [browser#1](https://github.com/devlanes-run/browser/issues/1) |
| `devlanes setup` registers only the browsers [Install devlanes](../install.md) lists. Another Chromium browser, such as Opera, needs the copy in `~/.devlanes/browser` linked into its `NativeMessagingHosts` folder by hand. | |
| A browser installed as a Snap or Flatpak can't start devlanes, so the extension can't reach it. | [browser#2](https://github.com/devlanes-run/browser/issues/2) |
| The extension only knows hostnames ending in `.localhost`, not a `hostnames.tld` such as `.test`. | [browser#5](https://github.com/devlanes-run/browser/issues/5) |
| Installs run with `uvx` or `pipx run` can't register the browser extension's host. Install with `uv tool install devlanes` or `pipx install devlanes`. | |
| The browser starts devlanes with its own environment, so a `DEVLANES_HOME` set only in a shell doesn't reach it and the extension uses the default home. | |
| With a very large number of lanes and hostnames, the popup can stop updating. devlanes can't send the extension a view over 1 MiB, so the popup keeps the last one. | |
| The login fill saves a login only when the browser fires a `submit` event, so a form sent with `form.submit()` or plain `fetch` isn't saved. | |
| A sign-in split across two pages is saved only when the password page keeps the username in a hidden field marked `autocomplete="username"` or `"email"`. | |
| A sign-in the site refuses is still saved, so a wrong password replaces the right one until the next sign-in that works. | |

## Remote boxes

| Limit | Issue |
|---|---|
| Only TCP ports are mirrored, not UDP. | |
| Connections run over SSH or a command; there is no HTTP or WebSocket transport. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| A mirrored port keeps the box's number; when the number is taken on your machine, the port is not mirrored. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| On Windows, each connection to a box signs in to SSH again. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| Boxes made from one image run the same lane, so your machine mirrors one of them at a time. | [#98](https://github.com/devlanes-run/devlanes/issues/98) |
| A box's unit logs are read on the box. | [#142](https://github.com/devlanes-run/devlanes/issues/142) |
| Windows of a desktop app or simulator are not streamed; a GUI tool reaches you only over HTTP or TCP. | [#30](https://github.com/devlanes-run/devlanes/issues/30) |
