---
name: devlanes-docs
description: Rules for the public documentation in docs/ and the agent skill in skills/devlanes/. Load before creating, editing, moving, deleting or reviewing any file under docs/ or skills/, and before finishing any change to a command, flag, config key or behaviour a devlanes user can notice, to decide whether the docs change and on which page.
---

# Writing the devlanes docs

`docs/` is the public documentation of devlanes. The website publishes it at https://devlanes.run/docs from the latest release tag. Nobody reads these files on GitHub: write for the rendered page.

`skills/devlanes/` is the skill people install into their coding agents. It follows its own rules, under "The public skill".

Read these before writing:

- [references/site-map.md](references/site-map.md): every page, its title, and the page that owns each fact.
- [references/page-kinds.md](references/page-kinds.md): a skeleton for each kind of page.
- [references/markdown.md](references/markdown.md): the directives the website renders, links and frontmatter.

## The idea

A page shows what to write and what to run, and says in a sentence what happens. That is all it does.

- Simple before complete. A reader who follows the page on an ordinary setup reaches the goal. Rare cases, quirks and how devlanes works inside are left out.
- Short. A reader's coding agent does most of the typing and reads the reference and the skill. The prose pages give a person a clear picture fast.
- Once. A fact lives on one page. Other pages link it or say nothing.

Most people try devlanes by pointing their agent at it, so the overview and the install page lead with a prompt to paste into a coding agent, and the manual steps follow.

## Before you write

1. Decide whether the docs change at all. A bug fix, a refactor or a new edge case usually changes nothing.
2. Find the page that owns the fact in the site map and edit there. When no page fits, add one; do not stretch a page or append to the nearest one.
3. Write it short, then cut: would a reader with this page's goal miss this sentence? If not, it goes.
4. A new page goes into `docs/nav.yaml` and the site map in the same commit. So does a rename, with every link to the page.
5. If the skill states the same fact, change the skill too.
6. Put a `Docs:` line in the pull request description: the pages changed, or `none` and the reason.

| The change | Where it goes |
|---|---|
| A command, flag or help text | The clap help in `crates/cli`. `docs/reference/cli/` is generated from it. |
| A config key, its default or its meaning | The doc comment in `crates/config`. The website builds the config reference from the schemas. |
| Something a user can now set out to do | A how-to guide. |
| A changed behaviour | The sentence that describes it, on the page that owns it. |
| A removed feature | Delete every mention. Do not say it was removed. |

## Page kinds

Every page has one kind, set in its frontmatter as `kind`.

- **Overview** (`docs/README.md`): one paragraph on what devlanes does, the prompt, one diagram, ways in by goal.
- **Learn** (`docs/learn/`): a short sequence read in order, following one project, myapp. Each page adds a few lines to the project file, runs a command or two, and says what happened. Nobody types along. About 400 words.
- **How-to guide** (`docs/how-to/`): one goal, named in the title with a verb. One sentence on when it applies, the smallest config that works, the commands in order, at most two "If ..." variations, a few related links. About 300 words.
- **Concept** (`docs/concepts/`): one part of devlanes explained as a model a person can reason with, with a diagram where it helps. No steps and no catalogue of exceptions. About 600 words.
- **Reference** (`docs/reference/`, `docs/errors/`, `troubleshooting.md`): what a person sets, types or sees, in tables.

A page that mixes kinds is how docs go wrong: explanation in a how-to guide, steps in a concept, option lists in a Learn page. Each moves to the page of its kind with a link left behind.

## What stays out

- Command output. A `console` block holds the commands a person types and nothing else. The reader runs them and sees the output.
- Internals: how devlanes decides something, timings, thresholds, what its processes send each other, files and variables it uses for itself, crate names.
- Edge cases and quirks: rare states, odd messages, what happens after a crash, traps on one OS that are off the main path. A fact stays, as one sentence, only when missing it loses someone's data or leaves them stuck on the main path.
- History and plans: no "now", "no longer", "new", "previously", no removed or renamed features, no roadmap. What devlanes lacks is one row on `reference/limits.md`.
- Caveats repeated on every page, and summaries of what another page says.
- Contributor material. That goes in `CONTRIBUTING.md`.

## Page shape

- The first paragraph says what the reader gets, in plain words, without talking about the page.
- A section is a heading, a short paragraph and one thing to look at: a config fragment, a command, a few lines of code or a diagram.
- Show the lines a step is about, as a fragment. Not the whole file.
- Commands go in `console` blocks so they can be copied. Config goes in `yaml devlanes` blocks.
- Code spans are for what a person types: a command, a key, a file name, a hostname. At most two in a paragraph.
- A term is defined in a clause where a page first uses it, or linked.
- Headings are sentence case and do not start with What, Why, Where or How.
- OS tabs only where the command or config differs by OS.
- Link text is the title of the page it opens, as the site map lists it.

## Voice

- "You", present tense. No "we", no "let's".
- Lead with the point. No preamble, no summary at the end.
- Plain words, and one word per thing.
- No em dashes, no "It's not X, it's Y", no rhetorical questions, no groups of three for rhythm, no filler ("note that", "simply", "just").
- No selling: seamless, powerful, effortless, robust, zero-config, unlock.
- No "any", "every", "always" or "never" unless it is true without exception.

## Words and names

| Use | For | Not |
|---|---|---|
| worktree | a Git worktree | working copy |
| lane | one worktree's place to run the project, with ports, hostnames and data of its own | environment, instance, slot, copy |
| project | what one `devlanes.yaml` describes | repo, workspace |
| project file | `devlanes.yaml` | config, manifest; and not `package.json` |
| user config | `~/.devlanes/config.yaml` | settings file |
| local file | `devlanes.local.yaml` | override file |
| unit | a service, task or command that a lane runs | process, job |
| service | a long-running process of a lane | server, app |
| container | something under `infra` | infra |
| ready check | `ready` | health check, probe |
| daemon | the background process that runs the lanes | server, agent |
| box | a machine whose lanes show up on another | remote, host, server |
| your machine | where the browser runs when a box is involved | client, laptop |
| CA | the certificate authority devlanes creates on your machine | authority, root |
| hostname | `api.myapp1.localhost` | domain, address |
| frozen, thaw | a lane nobody uses, suspended and running again | idle (a state of its own), resume (a command of its own) |

Names in examples: the project is `myapp` with services `api` and `web`. Lane domains are `myapp.localhost` and `myapp1.localhost`. Branches are `main` and `feat/checkout`. The box is `my-vps`. Folders are `~/dev/myapp` and `~/dev/myapp-checkout`. Outside hosts end in `example.invalid`.

## Accuracy

A wrong page is worse than a missing one, and a short page is easier to keep right.

- Every command and flag on a page exists in `devlanes --help`.
- A `yaml devlanes` block uses only keys the schema has, and a whole file passes `devlanes config check`.
- A claim about behaviour comes from the code or from running it, not from another page.
- A claim about another tool comes from that tool's documentation.
- A guide nobody has completed carries `publish: false`.

## Generated pages

- `docs/reference/cli/` and `docs/reference/cli/commands.json` come from the clap definitions through a test in `crates/cli`. Change the help text and regenerate.
- The config reference is not in this repository. The website renders it from `crates/config/schema.json` and `crates/config/user-schema.json`, so its text is the doc comments in `crates/config`.

## The public skill

An agent loads `skills/devlanes/SKILL.md` on any task in a repository that uses devlanes, so every line costs context on every task.

- `SKILL.md` covers daily use: running this worktree's lane, URLs, logs, tests, long-running commands, stopping what the agent started, states and exit codes.
- Setting devlanes up, writing `devlanes.yaml`, migrating, the MCP server, boxes and troubleshooting each live in a file under `references/`, named at the top of `SKILL.md` with the situation that calls for it.
- It is exact and terse: imperative sentences, tables, real commands. The skill may be more detailed than the docs, because an agent is the reader.
- An agent cannot open the TUI or the user's browser. The skill tells the agent what to ask the user to run or open.
- It links the docs by their Markdown twin, `https://devlanes.run/docs/<page>.md`.
