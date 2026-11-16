---
cargo/devlanes: patch
cargo/devlanes-platform-java: patch
cargo/devlanes-pki: patch
cargo/devlanes-core: patch
cargo/devlanes-resolve: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Added `platform: java`, so JVMs that devlanes starts, `devlanes exec` and `devlanes with` trust the devlanes CA.

devlanes detects it from a `pom.xml` or Gradle file and adds a trust store to `JAVA_TOOL_OPTIONS`. Keep JVM flags out of a `JAVA_TOOL_OPTIONS` in the project file, which would replace it: https://devlanes.run/docs/how-to/hostnames/trust-the-ca/

`devlanes env shell`, `env export`, `exec` and `with` now set `JAVA_TOOL_OPTIONS` with the devlanes trust store in every project, and keep a `javax.net.ssl.trustStore` you set yourself.
