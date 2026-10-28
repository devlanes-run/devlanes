---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Added `NODE_EXTRA_CA_CERTS` and `DENO_CERT` (the devlanes CA) and `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` (the CA bundle) to the environment of the command `devlanes with` runs, unless the shell already sets them, so tests trust the lane's `https://` URLs.
