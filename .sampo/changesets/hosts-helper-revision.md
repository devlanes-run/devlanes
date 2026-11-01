---
cargo/devlanes: patch
cargo/devlanes-hosts-helper: patch
cargo/devlanes-resolver-hosts: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.

Doctor's error code for an old helper is now `helper_revision_outdated`.
