---
cargo/devlanes: patch
cargo/devlanes-preset-spring-boot: patch
cargo/devlanes-config: patch
---

Added `preset: spring-boot`, which runs a Spring Boot app on its lane's port and hostname.

It also turns off Spring's own Docker Compose support, since devlanes runs the containers; `spring-boot: { docker_compose: true }` turns it back on.
