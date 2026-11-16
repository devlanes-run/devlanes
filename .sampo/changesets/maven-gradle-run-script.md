---
cargo/devlanes: patch
cargo/devlanes-run-script-maven: patch
cargo/devlanes-run-script-gradle: patch
cargo/devlanes-config: patch
---

Added Maven and Gradle to `run_script`: a `pom.xml` runs Maven goals and a Gradle file runs Gradle tasks.

`name` holds the arguments as typed after `mvn` or `gradle`, and devlanes uses the project's `mvnw` or `gradlew` when there is one:

```yaml
run_script: { file: pom.xml, name: spring-boot:run -Pdev }
```
