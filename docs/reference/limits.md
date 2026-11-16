---
title: Limits
description: What devlanes does not do, each with the issue that tracks it.
kind: reference
---

Each entry is something devlanes does not do, with the issue in `devlanes-run/devlanes` that tracks it. An empty Issue cell means no issue tracks the limit. [Windows](windows.md) lists what differs on Windows.

## Projects and services

| Limit | Issue |
|---|---|
| A project name belongs to one repository. A second repository that uses a taken name is refused with `project NAME is already used by PATH; set another project: in this file`. | |
| A variable cannot be unset: an `env` value is a string, number or boolean, and has no form that removes a variable a unit would inherit. | |
| A service has no `env_file`. `.env` values reach a unit only through `${{ env.NAME }}` expressions. | [#186](https://github.com/devlanes-run/devlanes/issues/186) |
| The user config has no values or secrets that every project can read. | [#137](https://github.com/devlanes-run/devlanes/issues/137) |
| Go has no platform and no script runner for `go.mod`, and a `Taskfile.yml` is refused with `no script runner handles <file>`. | [#138](https://github.com/devlanes-run/devlanes/issues/138) |
| JVM projects have no platform, no Maven or Gradle runner and no presets, and Java does not read the CA variables devlanes sets. | [#136](https://github.com/devlanes-run/devlanes/issues/136) |
| A service has no debug port of its own kind. A debugger port is a named port written into the command, and devlanes does not show it as a debugger. | [#139](https://github.com/devlanes-run/devlanes/issues/139) |
| Resources outside Docker, such as a hosted database branch, cannot be created per lane with a value fed back into the config. A start brings up every container of the lane whichever services it starts, unless its profiles turn containers off. | [#59](https://github.com/devlanes-run/devlanes/issues/59) |
| The daemon has no public API or SDK for other programs. | [#29](https://github.com/devlanes-run/devlanes/issues/29) |

## Hostnames, HTTPS and the proxy

| Limit | Issue |
|---|---|
| A lane's bare domain, such as `myapp1.localhost`, is refused at the TLS handshake when no service routes it. | [#100](https://github.com/devlanes-run/devlanes/issues/100) |
| The proxy speaks HTTP/1.1 to services, so gRPC over HTTP/2 does not go through a hostname. | |
| Hostnames resolve only through this machine's hosts file or resolver; no public DNS name points at a lane. devlanes refuses `hostnames.resolver: public`, `hostnames.base_domain` and `hostnames.dns_provider` in the user config. | |
| The CA is not installed in iOS simulators or Android emulators. `ios-simulator` and `android-emulator` in `tls.trust` are dropped with a warning. | |
| There is no way to share cookies, storage or saved passwords between lanes of one project. | [#190](https://github.com/devlanes-run/devlanes/issues/190) |
| The proxy adds no `Server-Timing` header for the time a request spent in devlanes. | [#141](https://github.com/devlanes-run/devlanes/issues/141) |

## Remote boxes

| Limit | Issue |
|---|---|
| Only TCP ports are mirrored. UDP ports, such as DNS servers, WebRTC, TURN and QUIC, are not. | |
| Connections run over SSH or a command that runs `devlanes tunnel --stdio`. There is no HTTP or WebSocket transport. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| Mirrored data streams are not compressed. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| A mirrored port keeps the box's number. When the number is taken on your machine, the port is not mirrored. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| On a Windows machine, each connection to a box signs in to SSH again. | [#46](https://github.com/devlanes-run/devlanes/issues/46) |
| Boxes made from one image run the same lane with the same hostnames and ports, so your machine mirrors one of them at a time. | [#98](https://github.com/devlanes-run/devlanes/issues/98) |
| A box's unit logs are read on the box. The TUI on your machine shows a box unit's details, not its output. | [#142](https://github.com/devlanes-run/devlanes/issues/142) |
| A window of a desktop app or simulator is not streamed. A GUI tool reaches your machine only when it serves HTTP or TCP on a port. | [#30](https://github.com/devlanes-run/devlanes/issues/30) |
