# devlanes-browser-host

## 0.3.0 — 2026-11-28

### Minor changes

- [4c06151c](https://github.com/devlanes-run/devlanes/commit/4c06151c0396f3ffa997e1b3db3ebf37fbd1ac4b) Changed the browser extension host to allow the extension id `konofbnhgejkgkonkleipkkpkphbfgee` in place of `pggffblellmckocpflaaeligadjpkahj`.
  
  Builds of devlanes-run/browser with the old id stop connecting, so load the latest build. Run `devlanes setup` again after updating.

## 0.2.0 — 2026-11-23

### Patch changes

- [71325d24](https://github.com/devlanes-run/devlanes/commit/71325d247eecd40c1dafa88a181c9121a0ee0ca4) Added browser extension support to `devlanes setup` for Chromium-based browsers such as Chrome, Edge and Brave.
  
  The extension doesn't keep the daemon running, so the daemon can stop while the popup is open.

