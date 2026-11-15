# Page kinds

A skeleton for each kind, then one dense paragraph rewritten. The skeletons show shape and order; the words in them are placeholders.

## Learn page

````markdown
---
title: Every worktree gets a lane
description: A second worktree of myapp starts next to the first, on its own ports and hostname.
kind: learn
---

One or two sentences: where myapp stands after the page before, and what this page adds.

## A heading that states the point

One or two short paragraphs. What the reader is about to look at, and the one new idea in it, named once.

```console title="~/dev/myapp-checkout"
$ devlanes start
myapp@1 (feat/checkout): 2/2 ready
ready api https://api.myapp1.localhost
ready web https://myapp1.localhost
```

One paragraph on what the artefact shows that the reader should notice.

## The next point

...

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Lanes and projects](../concepts/lanes-and-projects.md)
````

- The page follows `myapp` from the page before. It does not start a new story.
- Three to five sections. Each has one artefact.
- No instruction to the reader. The page shows a command and what came back; it never says to run it.
- One new idea per section, in one sentence. The concept page holds the rest.
- The last section is links only: the how-to guides that do this on the reader's project, and the concept page.

## How-to guide

````markdown
---
title: Share one Postgres between lanes
description: Run one Postgres container for every lane and give each lane its own database in it.
kind: how-to
---

One line: the situation this guide is for, and what you end up with.

::example{slug="infra-postgres-shared"}

## Add the container

A sentence or two, then the config.

```yaml devlanes
infra:
  compose: docker-compose.yml
```

## Create a database for each lane

...

## If each lane needs its own server

A short paragraph, and a link to the guide for that case.
````

- The title starts with a verb and names the goal, not the feature.
- Steps are sections in the order a person does them. Use the steps directive only for a short run of commands.
- No explanation of how things work. One clause of reason is enough ("so a migration on a branch leaves `main` alone"), with a link to the concept page.
- Variations come last, each under a heading that names the case.
- Keys and flags the guide does not need are not mentioned. The reference has them.

## Concept page

````markdown
---
title: Idle lanes
description: A lane nobody uses is slowed down, then suspended, and comes back on the next request.
kind: concept
---

Two or three sentences: what the thing is and the problem it answers.

## A heading that states the point

Prose. The reasoning, the trade-off, what was decided against and the reason.

![A lane moving between running, throttled and frozen](idle-lanes.svg)

## The next point

...

## Related

- [Run many lanes on one machine](../how-to/lanes/many-lanes.md)
- [`lanes.freeze_after`](/docs/reference/config/user/lanes/)
````

- No steps and no commands to run. A command may appear in an artefact that shows behaviour.
- Reasons and alternatives are the content. A concept page may hold an opinion.
- Settings are named in a link to the reference, not described.

## Reference page

````markdown
---
title: Unit states and exit codes
description: The states a unit can be in and the exit codes of the CLI.
kind: reference
---

One sentence saying what the page lists.

## Unit states

| State | Meaning |
|---|---|
| `ready` | The unit runs and passed its ready check. |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
````

- Describe; do not advise or explain reasons. Link a how-to guide or a concept for those.
- The same layout for every entry on the page.
- Complete. A reference page that lists some of the states is wrong.
- Order follows the product: the order of the config, of the help, of the screen.

## A dense paragraph, rewritten

This paragraph is correct, and no reader gets through it. It holds a lifetime rule, an idempotence rule, the reload model and the freezing model, with a setting name and two cross-references.

> The daemon starts on first use or with `devlanes daemon start`, and stops on its own about 15 seconds after the last TUI closes while nothing runs (the summary printed when the TUI closes says so), or after about 5 minutes when neither a TUI nor a command showing progress ever watched it, as when only scripts or agents used it (`daemon.exit_when_idle`). `start` is safe to repeat: running services are left alone. An edit to `devlanes.yaml` leaves them alone too: `devlanes status` shows which services it changed, added or removed, and `devlanes reload` restarts, starts or stops only those. A lane without requests, connections or commands for 15 minutes is frozen, and the next request, connection or command resumes it.

Where each fact goes:

| Fact | Page |
|---|---|
| The daemon starts on first use and outlives the terminal | `learn/first-service.md`, one sentence |
| It exits when idle, with the timings | `concepts/daemon.md`; the setting in the generated reference |
| Starting twice is safe | `learn/first-service.md`, one sentence |
| An edit never restarts anything; `reload` applies it | `learn/failures.md`, with an artefact; the model in `concepts/config-and-reload.md` |
| Idle lanes freeze and resume | `concepts/idle-lanes.md`; one sentence in `learn/worktrees.md` |

On the Learn page the first fact becomes a section:

````markdown
## The lane keeps running after you close the terminal

A background daemon owns the processes. The terminal you started from can close, and the page still loads.

```console
$ devlanes status
myapp
  LANE  BRANCH  STATE    SERVICES  URL
  0     main    running  1/1       https://myapp.localhost
```
````
