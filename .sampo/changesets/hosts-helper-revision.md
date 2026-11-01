---
cargo/devlanes: patch
cargo/devlanes-hosts-helper: patch
cargo/devlanes-resolver-hosts: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.

`devlanes doctor` and the hosts status now report an old helper as `helper_revision_outdated` instead of `helper_version_mismatch`.
