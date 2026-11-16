# devlanes-pki

## 0.5.0 — 2026-11-16

### Minor changes

- [a45c6921](https://github.com/devlanes-run/devlanes/commit/a45c69214d7111c1497394380bf4100e9458f27e) Added `platform: java`, so JVMs that devlanes starts, `devlanes exec` and `devlanes with` trust the devlanes CA.
  
  devlanes detects it from a `pom.xml` or Gradle file and adds a trust store to `JAVA_TOOL_OPTIONS`. Keep JVM flags out of a `JAVA_TOOL_OPTIONS` in the project file, which would replace it: https://devlanes.run/docs/how-to/hostnames/trust-the-ca/
  
  `devlanes env shell`, `env export`, `exec` and `with` now set `JAVA_TOOL_OPTIONS` with the devlanes trust store in every project, and keep a `javax.net.ssl.trustStore` you set yourself.
  
  <!-- highlight
  title: Run Java services
  body: Point `run_script` at a `pom.xml` or Gradle file, and add `preset: spring-boot` or `preset: quarkus` to give each lane its own ports. JVMs that devlanes starts trust its CA.
  -->

## 0.4.0 — 2026-11-07

### Patch changes

- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Fixed new local CAs on Windows being labelled `localhost` instead of the computer's name.

