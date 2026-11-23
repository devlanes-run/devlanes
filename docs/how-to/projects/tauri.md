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

The app's window opens once the first Rust build finishes, which takes longer than the start itself. Each lane and each copy builds the app in its own folder, so the first start of each is a full Rust build.

To choose the build folder yourself, set `CARGO_TARGET_DIR` in the app's `env`. The folder has to differ per lane and copy:

```yaml devlanes
services:
  desktop:
    env:
      CARGO_TARGET_DIR: "${{ lane.data_path }}/target/${{ replica.index }}"
```

## If you want several copies in one lane

Set `replicas: 2` on the app. The replica number in the identifier keeps the copies' data apart, and [Run workers and replicas](../services/workers.md) has more on replicas.

## Related

- [Run Vite and Next.js](vite-and-next.md)
- [Run a folder as two lanes](../lanes/two-lanes-one-folder.md)
- [Limits](../../reference/limits.md#remote-boxes)
