---
cargo/devlanes: minor
cargo/devlanes-resolve: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
---

Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost` instead of `acme-2.localhost`.

Lane 0 keeps `acme.localhost`, and the `resolver` strategy gives `acme2.test`. A project that sets `lanes.vars.domain` keeps its own. Replace `acme-2.localhost` wherever you wrote it, such as `.env` files and OAuth redirect URIs. Lane 2 of `acme` and lane 0 of a project named `acme2` now share `acme2.localhost`, and devlanes warns about it with `host_conflict`.
