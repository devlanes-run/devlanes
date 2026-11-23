---
title: Run a Tauri app
description: Run a Tauri 2 desktop app for each lane, or several copies in one lane, each with its own app data and its own lane's frontend.
kind: how-to
publish: false
---

Use this guide when a project has a Tauri 2 desktop app and you want one running per lane, or several copies in one lane, without them sharing data or loading each other's frontend.

## Add the frontend and the app

The frontend runs as its own service, so each lane serves it on its own port. The tauri preset gives each copy of the app its own config, merged over `tauri.conf.json` without changing the repository:

```yaml devlanes
services:
  desktop-web:
    preset: vite
    run_script:
      file: apps/desktop/package.json
      name: dev:web
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

The identifier has to change with the lane and the copy, because Tauri keeps the app's data under it; a start fails when two copies would share one. The dev URL points each app at its own lane's frontend, and the empty command stops Tauri from starting a frontend of its own. Keep the `dev` script a plain `tauri dev`, since the preset adds its flag at the end.

## Start the lane

```console title="~/dev/myapp"
$ devlanes start
```

The app's window opens once the first Rust build finishes, which takes longer than the start itself.

## If you want several copies in one lane

Set `replicas: 2` on the app. The replica number in the identifier keeps the copies' data apart, and each copy after the first builds the app in a folder of its own, so its first start is a full Rust build. [Run workers and replicas](../services/workers.md) has more on replicas.

A second lane in the same folder builds the app where the first one does; point the app's `CARGO_TARGET_DIR` at the lane's data folder as [Run a folder as two lanes](../lanes/two-lanes-one-folder.md) shows.

## Related

- [Run Vite and Next.js](vite-and-next.md)
- [Run a folder as two lanes](../lanes/two-lanes-one-folder.md)
- [Limits](../../reference/limits.md#remote-boxes)
