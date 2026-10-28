---
cargo/devlanes: minor
cargo/devlanes-resolve: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
---

Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost` instead of `acme-2.localhost`.

Lane 0 keeps `acme.localhost`, and the `resolver` strategy gives `acme2.test`. Update bookmarks, OAuth redirect URIs registered for lanes, and literal domains in `.env` files. A project that sets `lanes.vars.domain` keeps its own.
