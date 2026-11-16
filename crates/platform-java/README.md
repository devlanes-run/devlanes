# devlanes-platform-java

Launch contributor for Java units. The binary registers it and `devlanes-resolve` applies it to units whose platform is `java`.

## Usage

```rust
use std::sync::Arc;
use devlanes_core::contrib::Contributor;
use devlanes_platform_java::JavaContributor;

let java: Arc<dyn Contributor> = Arc::new(JavaContributor);
// Pass in `ResolveInput::contributors`.
```

## Config example

```yaml
services:
  api:
    run_script: { file: pom.xml, name: spring-boot:run }   # platform inferred
  worker:
    platform: java
    run: java -jar build/libs/worker.jar
```

Java units get:

- `JAVA_TOOL_OPTIONS` with `-Djavax.net.ssl.trustStore` pointing at `truststore.p12`, a PKCS#12 store with the system roots plus the devlanes root. The flags are appended to the value from mise, direnv or the client. When that value already sets `javax.net.ssl.trustStore`, it is kept and the unit shows a note.
- For the Gradle runner, `--no-daemon` on services, so the application JVM is a descendant of the unit that stop, freeze and metrics can see, and `--console=plain` without a pty. Neither is added when the script name already has `--daemon`, `--no-daemon` or a `--console` option.
- For the Maven runner without a pty, `-B -ntp`.

## Behaviour

Without `platform`, a unit is claimed when its `run_script` source is `pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle` or `settings.gradle.kts`, or its working directory contains one. A directory with a Java build file next to `package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml` or `requirements.txt` keeps that other platform.

Every JVM prints `Picked up JAVA_TOOL_OPTIONS: ...` on stderr when the variable is set.

## License

Apache-2.0
