---
cargo/devlanes: patch
cargo/devlanes-resolver-dns: patch
---

Fixed the error when the `hostnames.resolver` DNS server cannot bind its port: it now says whether UDP or TCP failed, and on Windows it gives the TCP lookup commands when TCP is the one held.
