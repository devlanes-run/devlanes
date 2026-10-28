---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes certs trust --hosts` and `devlanes doctor` on Windows claiming that Windows resolves every `*.localhost` name; they now report what this machine's lookup found, and when it fails (as on Windows Server) they say to add the hostnames to the hosts file from an elevated shell or use the resolver strategy.
