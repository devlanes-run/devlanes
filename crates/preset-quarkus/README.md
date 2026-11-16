# devlanes-preset-quarkus

`preset: quarkus` binds Quarkus dev mode to the service's allocated ports on loopback. Lane resolution applies it through `QuarkusPreset`, a `devlanes-core` `Contributor`; the composition root registers it and feeds `options_schema()` to the config.

## Usage

```rust
use devlanes_preset_quarkus::contribution;

let added = contribution(20013, 20014);
assert_eq!(added.args, ["-Ddebug=20014", "-Dquarkus.analytics.disabled=true"]);
assert_eq!(added.env["QUARKUS_HTTP_PORT"], "20013");
```

## API

- `QuarkusPreset`: the contributor; only acts on services with `preset: quarkus` and asks for their `debug` port.
- `QuarkusOptions`: the `quarkus:` options, empty for now; unknown keys are rejected.
- `parse_options` and `options_schema`: evaluated JSON to options, and the JSON Schema for config.
- `contribution`: the variables and build-tool arguments for a port and a debug port.

## Config example

```yaml
services:
  api:
    preset: quarkus
    run_script:
      file: build.gradle.kts
      name: quarkusDev
```

The preset sets `QUARKUS_HTTP_PORT`, `QUARKUS_HTTP_HOST=127.0.0.1` and `QUARKUS_HTTP_TEST_PORT=0` (continuous testing picks a free port instead of 8081). With a hostname, it also sets `QUARKUS_HTTP_HOST_VALIDATION_ALLOWED_HOSTS` to the service's public host, `localhost` and `127.0.0.1`: Quarkus 3.38 and later on a loopback address answers 400 to any other Host header, which is every request through the proxy. It allocates `self.ports.debug` and passes `-Ddebug=<port>` to Maven or Gradle, so dev mode's debugger does not fight over 5005 between lanes. When the `run_script` name already has a `-Ddebug` (`-Ddebug=false`, say), the preset leaves it alone and adds no port. It also passes `-Dquarkus.analytics.disabled=true`, because the first dev-mode run otherwise waits for an answer to the analytics question.

## Requires the Maven or Gradle runner

The debug port is a build-tool argument, so the service must use `run_script` with a `pom.xml` or `build.gradle(.kts)`. Any other runner fails the launch with an error.

## License

Apache-2.0
