---
title: Run Spring Boot and Quarkus
description: Put a Spring Boot or Quarkus app on its lane's port and hostname, started through Maven or Gradle.
kind: how-to
---

Use this guide when a Java service runs on Spring Boot or Quarkus and each lane should run its own copy under the lane's hostname.

## Add the preset

A preset passes the lane's port to the framework and binds the app to 127.0.0.1. Name it on the service, next to the Maven goal or Gradle task that starts the app:

::::tabs
:::tab[Spring Boot]
```yaml devlanes
services:
  api:
    preset: spring-boot
    run_script:
      file: pom.xml
      name: spring-boot:run
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /actuator/health
        timeout: 180s
```

The preset also lets Spring trust the proxy's forwarded headers, so links the app builds use the lane's `https://` hostname, and turns off DevTools LiveReload, whose fixed port would collide between lanes. With Gradle, the file is `build.gradle.kts` and the name `bootRun`.
:::
:::tab[Quarkus]
```yaml devlanes
services:
  api:
    preset: quarkus
    run_script:
      file: build.gradle.kts
      name: quarkusDev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /q/health/ready
        timeout: 180s
```

The preset also moves the dev-mode debugger from port 5005 to a port of each lane, `self.ports.debug`, which dev mode prints when it starts, and lets continuous testing pick a free port. It needs a `pom.xml` or Gradle file in `run_script`. With Maven, the name is `quarkus:dev`.

Quarkus answers 400 to a request for a hostname it does not know, so the preset allows the service's first hostname. A service with more hostnames lists them all, with `localhost` and `127.0.0.1`, in `QUARKUS_HTTP_HOST_VALIDATION_ALLOWED_HOSTS` in its `env`.
:::
::::

The health paths come from Spring Boot Actuator and the SmallRye Health extension; without them, check a route of your app. The first start downloads dependencies, so the check waits longer than the default. `name` is what you would type after `mvn` or `gradle`, as [Run scripts your project already has](run-scripts.md) shows.

## Start the lane

```console title="~/dev/myapp"
$ devlanes start
```

The start runs the project's Maven or Gradle wrapper and lists the app on the lane's hostname, such as `https://api.myapp.localhost`. The app trusts the devlanes CA when it calls another lane hostname.

## Run one module of a multi-module build

To build the modules an app needs from the root pom, name the app's module with `-pl` and add `-am`. Maven then runs the goal in every module it builds, so the parent pom skips the plugin and the app's module turns it back on:

```xml title="pom.xml"
<plugin>
  <groupId>org.springframework.boot</groupId>
  <artifactId>spring-boot-maven-plugin</artifactId>
  <configuration>
    <skip>true</skip>
  </configuration>
</plugin>
```

Put the same plugin with `<skip>false</skip>` in `web/pom.xml`, then point the service at the root pom:

```yaml devlanes
services:
  web:
    preset: spring-boot
    run_script:
      file: pom.xml
      name: -pl web -am spring-boot:run
```

Give `-am` to one service only. Maven has no build lock, so two services that build the same module at once write over each other's files.

## Let devlanes run the containers

devlanes runs the project's containers once for every lane or once per lane, as [Give each lane its own container](../infra/container-per-lane.md) shows. The frameworks' own container support would get in the way:

- Spring Boot: the preset sets `SPRING_DOCKER_COMPOSE_ENABLED=false`, because `spring-boot-docker-compose` would start a second copy of the Compose file's containers for each worktree. When devlanes does not run those containers, turn it back on with `spring-boot: { docker_compose: true }` on the service.
- Quarkus: Dev Services such as Kafka and Keycloak share one container between every app in dev mode, so two lanes would use the same one. Turn them off and point Quarkus at a container under `infra`:

```yaml devlanes
infra:
  compose: docker-compose.yml
  per_lane: [postgres]

services:
  api:
    env:
      QUARKUS_DEVSERVICES_ENABLED: "false"
      QUARKUS_DATASOURCE_JDBC_URL: jdbc:postgresql://127.0.0.1:${{ infra.postgres.port }}/myapp
```

## If Actuator listens on its own port

Name a `management` port on the service, and the preset sets Spring's management port to it, bound to 127.0.0.1:

```yaml devlanes
services:
  api:
    hosts:
      - "api.${{ lane.domain }}"
      - host: "actuator.${{ lane.domain }}"
        port: management
```

The HTTP ready check asks the main port, so point it at a route of your app.

## If you start a jar with run

A preset needs `run_script`. For a command such as `java -jar`, leave the preset out and set its variables yourself:

```yaml devlanes
services:
  api:
    run: java -jar target/api.jar
    env:
      SERVER_PORT: ${{ self.port }}
      SERVER_ADDRESS: 127.0.0.1
      SERVER_FORWARD_HEADERS_STRATEGY: framework
      SPRING_DOCKER_COMPOSE_ENABLED: "false"
```

A Quarkus jar takes `QUARKUS_HTTP_PORT`, `QUARKUS_HTTP_HOST` and `QUARKUS_HTTP_HOST_VALIDATION_ALLOWED_HOSTS` instead.

## Related

- [Run scripts your project already has](run-scripts.md)
- [Run a service in its runtime](runtimes.md)
- [Trust the devlanes CA](../hostnames/trust-the-ca.md)
- [Give a service several ports and hostnames](../services/ports-and-hostnames.md)
