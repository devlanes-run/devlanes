---
title: Run a Tauri app
description: Open a Tauri 2 desktop app for each lane, or several windows in one lane, each with its own app data and its own lane's frontend.
kind: how-to
---

Use this guide when a project has a Tauri 2 desktop app and you want one running per lane, or several copies in one lane, without them sharing data or loading each other's frontend.

## Run the frontend as its own service

The app's web frontend becomes a service of its own, so each lane serves it on its own port. For a Vite frontend that is the vite preset from [Run Vite and Next.js](vite-and-next.md):

```yaml devlanes
services:
  desktop-web:
    preset: vite
    run_script:
      file: apps/desktop/package.json
      name: dev:web
```

It needs no hostname, since only the app on this machine loads it.

## Add the app with the tauri preset

The tauri preset writes a small Tauri config for each lane and replica into the lane data folder, a folder outside the worktree that only one lane uses. It passes that file to `tauri dev`, and Tauri merges it over `tauri.conf.json`, so nothing in the repository changes.

```yaml devlanes
services:
  desktop:
    preset: tauri
    run_script:
      file: apps/desktop/package.json
      name: dev
    tauri:
      identifier: "com.example.myapp.lane${{ lane.index }}.client${{ replica.index }}"
      dev_url: "${{ services.desktop_web.url }}"
      before_dev_command: ""
```

The dev URL points each lane's app at the frontend of the same lane; the service name is written with an underscore in the expression. The empty command stops Tauri from starting a frontend of its own on the fixed port from `tauri.conf.json`.

The app listens on no port devlanes routes, so the start counts it as ready shortly after the process runs, well before the first Rust build opens a window. [Wait until a service is ready](../services/ready-checks.md) shows how to make the start wait.

## Give each copy its own identifier

Tauri keys the app data, the webview data and keychain entries by the bundle identifier. Two copies with one identifier would read and write the same data, so the identifier has to change with the lane and the replica. devlanes refuses to start a copy whose identifier another copy already uses, in any lane of the project.

The identifier may hold letters, digits, dots and hyphens, with no empty part between dots. To keep lane 0 on the identifier your team already uses, make the lane part conditional:

```yaml devlanes
services:
  desktop:
    tauri:
      identifier: "com.example.myapp${{ '' if lane.index == 0 else '.lane' ~ lane.index }}.client${{ replica.index }}"
```

## Open several windows in one lane

Replicas run more than one copy of a service in the same lane, which for a desktop app means several windows, for example two users of a chat app side by side. The replica number in the identifier keeps their data apart:

```yaml devlanes
services:
  desktop:
    replicas: 2
    tauri:
      product_name: "myapp ${{ lane.index }}-${{ replica.index }}"
```

The product name replaces the one from `tauri.conf.json` for that copy, so the copies carry different names. [Run workers and replicas](../services/workers.md) has more on replicas.

## If a wrapper script starts tauri dev

A script that calls `tauri dev` through a wrapper may not pass extra arguments on. The path of the generated config is also in the `DEVLANES_TAURI_CONFIG` variable, so the wrapper can pass that file to Tauri itself.

## If the lane runs on a box

The app opens on the machine that runs the lane, and a box does not send windows to your machine. [Limits](../../reference/limits.md) lists this.

## Related

- [Run a folder as two lanes](../lanes/two-lanes-one-folder.md)
- [Files and folders](../../reference/files.md)
- [Lanes and projects](../../concepts/lanes-and-projects.md)
