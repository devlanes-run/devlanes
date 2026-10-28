# devlanes-resolver-dns

## 0.4.0 — 2026-10-28

### Patch changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed the error when the `hostnames.resolver` DNS server cannot bind its port: it now says whether UDP or TCP failed, and on Windows it gives the TCP lookup commands when TCP is the one held.

