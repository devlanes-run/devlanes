---
title: Use the browser extension
description: See your lanes and their URLs in Chromium-based browsers such as Chrome, Edge and Brave, open a page in another lane, and pick which box serves a lane.
kind: how-to
---

The devlanes browser extension lists your projects and lanes with their URLs in a popup, and the toolbar badge shows the lane number of the current tab. It works in Chromium-based browsers such as Chrome, Edge and Brave.

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

Setup registers devlanes as the extension's native host. Reopen the popup afterwards. The browser doesn't need a restart.

## Open the popup

Click the devlanes icon in the toolbar. The popup shows whether the daemon runs, then each project with its lanes, their branch, state and routes. Click a route to open it. A wildcard route, such as `*.myapp.localhost`, has no single page, so it shows as text. The lane of the current tab is marked "This tab".

On a lane's page, the badge shows the lane number. It has a dot and turns blue when a box serves the lane.

The extension doesn't keep the daemon running, so the daemon can stop while the popup is open, as the TUI's exit summary says. Run `devlanes` in a terminal to start it again.

## Open this page in another lane

On a lane's page, open the popup and pick another lane of the project under "Open this page in". The extension opens the same service on that lane and keeps the path, query and hash. On a wildcard hostname, such as a tenant page, it keeps the tenant.

## Switch which box serves a lane

When more than one machine has a lane, such as your machine and a box, the lane shows a "Served by" row with "This machine" and each box. Pick the one that should serve the lane. [Work with several boxes](remote/several-boxes.md) covers what that changes.

When the lane runs on your machine, the popup asks before it stops it. "Stop and switch" stops the lane and then switches, which can take up to 30 seconds. When the lane's containers hold its ports, the popup lists the commands that stop them, each with a copy button. Run them, then click "Switch again".

## Fill test logins on other lanes

When you sign in with a form on a lane, the extension saves the username and password for the project. On any lane of the project, focus a field marked as a username, an email or a current password, and pick a login from the menu, with the mouse or the arrow keys and Enter. The extension fills both fields. The fill works on HTTPS lane pages.

:::caution
Passwords are stored unencrypted in this browser. Use the fill for test accounts only.
:::

The fill reads only fields marked with the standard `autocomplete` tokens `username`, `email`, `current-password` and `new-password`. It never saves one-time codes or card numbers.

## Change the login settings

Click "Login settings", the sliders button in the popup's header. Both switches are on until you turn them off.

- "Fill logins on lanes" turns the menu on or off.
- "Remember passwords" turns saving on or off. The logins already saved stay.
- "Saved logins" lists each project with its count. Click "Delete" twice to delete a project's logins.

A project keeps 20 logins, and the one used least recently goes first. [Limits](../reference/limits.md#browser-extension) lists the sign-ins the fill doesn't save, and [Security](../concepts/security.md#the-browser-extension-holds-test-passwords) says what a lane page can do with the saved logins.

## If a sign-in form gets no menu

The form's fields need `autocomplete` tokens.

```html
<input type="email" name="email" autocomplete="username">
<input type="password" name="password" autocomplete="current-password">
```

These guides cover the tokens and sign-in forms:

- [Autofill in the HTML standard](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#autofill)
- [The autocomplete attribute on MDN](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Attributes/autocomplete)
- [Sign-in form best practices](https://web.dev/articles/sign-in-form-best-practices)
- [Sign-up form best practices](https://web.dev/articles/sign-up-form-best-practices)
- [Create amazing password forms](https://www.chromium.org/developers/design-documents/create-amazing-password-forms/)

## Related

- [Install devlanes](../install.md)
- [Work with several boxes](remote/several-boxes.md)
- [Limits](../reference/limits.md)
