# Configuration examples

Configuration examples for a made-up `acme` project. They describe the v1 contract; the application files and Compose files they mention are not included. Runnable fixture projects are under `fixtures/`.

## Usage

- `minimal.yaml`: shared Postgres, per-lane database, API and web app.
- `devlanes.yaml`: reference forms, templates, tasks, service presets and profiles, with a comment on each section.
- `devlanes.local.yaml`: personal overrides merged over `devlanes.yaml`.
- `devlanes.prod.yaml`: a second project, `acme-prod`, that extends `devlanes.yaml` and runs its apps the production way.
- `user-config.yaml`: per-user settings for `~/.devlanes/config.yaml`.

`devlanes-config` tests parse every file here, including `devlanes.local.yaml` merged over `devlanes.yaml` and `devlanes.prod.yaml` loaded over both, so the examples stay valid.

## Behaviour

Map keys merge; lists and scalars replace. Files a project file `extends` merge before it, each with its own `.local.yaml`. Merge precedes template expansion. A `.local.yaml` file is normally gitignored; this documentation copy is intentionally tracked.

## License

Apache-2.0
