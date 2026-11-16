# devlanes-preset-spring-boot

`preset: spring-boot` binds a Spring Boot app to the service's allocated port on loopback through environment variables. Lane resolution applies it through `SpringBootPreset`, a `devlanes-core` `Contributor`; the composition root registers it and feeds `options_schema()` to the config.

## Usage

```rust
use devlanes_preset_spring_boot::{SpringBootOptions, contribution};
use std::collections::BTreeMap;

let env = contribution(20013, &BTreeMap::new(), &SpringBootOptions::default());
assert_eq!(env["SERVER_PORT"], "20013");
assert_eq!(env["SPRING_DOCKER_COMPOSE_ENABLED"], "false");
```

## API

- `SpringBootPreset`: the contributor; only acts on services with `preset: spring-boot`.
- `SpringBootOptions`: the `spring-boot:` options; unknown keys are rejected. `merge` applies overlays.
- `parse_options` and `options_schema`: evaluated JSON to options, and the JSON Schema for config.
- `contribution`: the variables for a port and the service's named ports, without a launch draft.

## Config example

```yaml
services:
  api:
    preset: spring-boot
    run_script:
      file: pom.xml
      name: spring-boot:run
    hosts:
      - api.${{ lane.domain }}
      # Referencing `management` moves actuator to its own port.
      - host: actuator.${{ lane.domain }}
        port: management
```

The preset sets `SERVER_PORT`, `SERVER_ADDRESS=127.0.0.1`, `SERVER_FORWARD_HEADERS_STRATEGY=framework` and `SPRING_DEVTOOLS_LIVERELOAD_ENABLED=false`. It adds `MANAGEMENT_SERVER_PORT` and `MANAGEMENT_SERVER_ADDRESS` only when the service has a `management` named port. It sets `SPRING_DOCKER_COMPOSE_ENABLED=false` unless `spring-boot: { docker_compose: true }`, because devlanes runs the project's containers and Spring's Compose support would start a second copy per worktree. Variables in the service's own `env` win over the preset's.

## License

Apache-2.0
