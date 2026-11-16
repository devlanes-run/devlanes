# devlanes-preset-spring-boot

## 0.8.0 — 2026-11-16

### Patch changes

- [a45c6921](https://github.com/devlanes-run/devlanes/commit/a45c69214d7111c1497394380bf4100e9458f27e) Added `preset: spring-boot`, which runs a Spring Boot app on its lane's port and hostname.
  
  It also turns off Spring's own Docker Compose support, since devlanes runs the containers; `spring-boot: { docker_compose: true }` turns it back on.

