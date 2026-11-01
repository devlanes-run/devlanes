---
cargo/devlanes: patch
cargo/devlanes-resolver-hosts: patch
---

Fixed `devlanes setup` failing to install the /etc/hosts helper on machines whose sudo configuration already had an unrelated problem, such as GitHub's Ubuntu runners; it now installs the helper and warns about the other file.
