---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
---

Fixed commands failing with `not found` in another project's folder or checkout when `DEVLANES_PROJECT` was inherited, such as a CI step after the GitHub action exported a lane's variables: an inherited `DEVLANES_PROJECT` now applies only inside its project's folder or outside every project, while `-p` still always applies.
