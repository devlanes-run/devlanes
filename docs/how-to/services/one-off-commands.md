---
title: Run a command with a lane's environment
description: Run a database reset, a seed or a script with a lane's variables without starting the lane's services.
kind: how-to
---

Use this when a command that ends by itself, such as a database reset, needs the lane's variables but not its services. A command that needs the services running goes through `devlanes with`, as [Run tests against a lane](../ci/tests.md) shows.

## Start the containers the command reads

Neither way below starts a container. When the command reads a variable built from one, such as a database URL, start the containers first:

```console title="~/dev/myapp-checkout"
$ devlanes infra up
```

## Declare a command you run often as a task

A task without `when` runs only when you name it:

```yaml devlanes
tasks:
  db-reset:
    run_script:
      file: package.json
      name: db:reset
```

```console title="~/dev/myapp-checkout"
$ devlanes run db-reset
```

The script runs with the variables of lane 1, starts no service, and the command exits with the script's exit code. [Run installs, migrations and seeds](tasks.md) covers the rest of a task.

## Run any other command with exec

Give the command a name and put it after the two dashes:

```console title="~/dev/myapp-checkout"
$ devlanes exec --name reset -- npx prisma migrate reset --force
$ devlanes wait reset
$ devlanes logs reset
```

The first command hands the reset to the daemon and returns at once. The second returns when the reset has ended, with exit code 0 when it succeeded and 1 when it did not. The third prints its output.

The command gets what `global_env` sets and a `DEVLANES_PUBLIC_URL_<SERVICE>` variable with the URL of each service. The settings of a single service do not reach it.

## If you want the variables in your own terminal

```console title="~/dev/myapp-checkout"
$ devlanes env shell
```

This opens a shell with the lane's variables, where you run the command yourself. `exit` leaves it.

## Related

- [Run installs, migrations and seeds](tasks.md)
- [Run tests against a lane](../ci/tests.md)
- [Environment and expressions](../../concepts/environment.md)
- [`devlanes exec`](../../reference/cli/exec.md)
