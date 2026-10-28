---
cargo/devlanes: patch
cargo/devlanes-config: patch
cargo/devlanes-resolve: patch
---

Added the `domain(name)` expression function, which gives a name its own site in each lane, such as `pages2.localhost` in lane 2, for user content that must not share the app's cookies.
