---
cargo/devlanes: minor
cargo/devlanes-platform-java: minor
cargo/devlanes-pki: minor
cargo/devlanes-core: minor
cargo/devlanes-resolve: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
---

Added `platform: java`, so JVMs that devlanes starts, `devlanes exec` and `devlanes with` trust the devlanes CA.

devlanes detects it from a `pom.xml` or Gradle file and adds a trust store to `JAVA_TOOL_OPTIONS`. Keep JVM flags out of a `JAVA_TOOL_OPTIONS` in the project file, which would replace it: https://devlanes.run/docs/how-to/hostnames/trust-the-ca/

`devlanes env shell`, `env export`, `exec` and `with` now set `JAVA_TOOL_OPTIONS` with the devlanes trust store in every project, and keep a `javax.net.ssl.trustStore` you set yourself.

<!-- highlight
title: Run Java services
body: Point `run_script` at a `pom.xml` or Gradle file, and add `preset: spring-boot` or `preset: quarkus` to give each lane its own ports. JVMs that devlanes starts trust its CA.
-->
