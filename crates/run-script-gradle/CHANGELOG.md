# devlanes-run-script-gradle

## 0.8.0 — 2026-11-16

### Patch changes

- [a45c6921](https://github.com/devlanes-run/devlanes/commit/a45c69214d7111c1497394380bf4100e9458f27e) Added Maven and Gradle to `run_script`: a `pom.xml` runs Maven goals and a Gradle file runs Gradle tasks.
  
  `name` holds the arguments as typed after `mvn` or `gradle`, and devlanes uses the project's `mvnw` or `gradlew` when there is one:
  
  ```yaml
  run_script: { file: pom.xml, name: spring-boot:run -Pdev }
  ```

