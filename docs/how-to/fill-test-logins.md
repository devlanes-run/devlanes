---
title: Fill test logins on lanes
description: Save the test logins you use on one lane and fill them on the project's other lanes.
kind: how-to
---

Use this when your lanes share test accounts and you sign in on each lane again. The fill is part of the devlanes browser extension, which [Use the browser extension](use-the-browser-extension.md) sets up.

## Fill test logins on other lanes

When you sign in with a form on an HTTPS lane page, the extension saves the username and password for the project. On any lane of the project, focus a field marked as a username, an email or a current password, and pick a login from the menu with the mouse, or with the arrow keys and Enter. The extension fills both fields.

:::caution
Passwords are stored unencrypted in this browser. Use the fill for test accounts only.
:::

The fill reads only fields with a standard `autocomplete` token for a username, an email or a password, and never saves one-time codes or card numbers.

## Change the login settings

Click "Login settings", the sliders button in the popup's header. Both switches start on.

- "Fill logins on lanes" turns the menu on or off.
- "Remember passwords" turns saving on or off, and keeps the logins already saved.

"Saved logins" lists each project with its count. Click "Delete", then click it again within 4 seconds to delete a project's logins.

A project keeps 20 logins, one per username. A new password for a username replaces the old one on every lane, and a 21st login replaces the one used least recently. [Limits](../reference/limits.md#browser-extension) lists the sign-ins the fill doesn't save, and [Security](../concepts/security.md#the-browser-extension-holds-test-passwords) says what a lane page can do with them.

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

- [Use the browser extension](use-the-browser-extension.md)
- [Limits](../reference/limits.md)
- [Security](../concepts/security.md)
