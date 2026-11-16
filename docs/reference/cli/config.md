---
title: devlanes config
description: Validate devlanes.yaml, or read and change the user config.yaml.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

`check` validates the project's devlanes.yaml in this worktree. `get`, `set` and `unset` act on the user config.yaml in the devlanes home (\~/.devlanes, or $DEVLANES_HOME), the settings this machine shares across projects; they work without the daemon.

```text
devlanes config [OPTIONS] <COMMAND>
```

```text
Examples:
  devlanes config check                     validate this worktree's devlanes.yaml
  devlanes config get                       every user setting, defaults included
  devlanes config get proxy.https_port
  devlanes config set tui.theme dark
  devlanes config set tls.trust '[system, firefox]'
  devlanes config unset tui.theme           back to the default
```

## devlanes config check

Validate this worktree's devlanes.yaml, including preset options.

```text
devlanes config check [OPTIONS]
```

## devlanes config get

Print a setting of the user config.yaml with defaults included; without a key, every setting.

```text
devlanes config get [OPTIONS] [KEY]
```

- `[KEY]`: Dotted setting or section, such as tui.theme or proxy [default: the whole config].

## devlanes config set

Change a setting in the user config.yaml, keeping its comments; refuses invalid values.

```text
devlanes config set [OPTIONS] <KEY> <VALUE>
```

- `<KEY>`: Dotted setting, such as tui.theme or proxy.https_port.
- `<VALUE>`: New value, read as YAML: true, 443, dark, 30d or [system, firefox].

## devlanes config unset

Remove a setting from the user config.yaml so its default applies again.

```text
devlanes config unset [OPTIONS] <KEY>
```

- `<KEY>`: Dotted setting or section, such as tui.theme.

Every command also takes the [global options](README.md#global-options).
