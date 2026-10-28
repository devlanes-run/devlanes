---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes certs trust --hosts` and `devlanes doctor` on Windows assuming Windows resolves every `*.localhost` name; when the lookup fails, as on Windows Server, they now say to add the hostnames to the hosts file from an elevated shell or use the resolver strategy.
