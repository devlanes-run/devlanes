---
name: devlanes-docs
description: Rules for the public documentation in docs/ and the agent skill in skills/devlanes/. Load before creating, editing, moving, deleting or reviewing any file under docs/ or skills/, and before finishing any change to a command, flag, config key, output or behaviour a devlanes user can notice, to decide whether the docs change and on which page.
---

# Writing the devlanes docs

`docs/` is the public documentation of devlanes. The website publishes it at https://devlanes.run/docs from the latest release tag, so a change shows up with the next release. Nobody reads these files on GitHub: write for the rendered page.

`skills/devlanes/` is the skill people install into their coding agents with `npx skills add devlanes-run/devlanes --skill devlanes`. It follows its own rules, under "The public skill".

Read these before writing:

- [references/site-map.md](references/site-map.md): every page, what it holds, and the page that owns each fact.
- [references/page-kinds.md](references/page-kinds.md): a skeleton for each kind of page, and a dense paragraph rewritten.
- [references/markdown.md](references/markdown.md): the directives the website renders, links and frontmatter.
- The `writing-whip` skill when your agent has it. The rules under "Voice" are the short form.

## Readers

A developer opens the docs to decide whether devlanes fits, to get one thing working, or to look one thing up. Their coding agent does most of the typing: it reads the skill and the reference, runs the commands and writes the config. So the prose pages are there to give a person a clear picture fast, and the reference and the skill are there to be exact.

Most people try devlanes by pointing their agent at it. So the overview and the install page lead with that path: a prompt to paste into a coding agent, which installs the skill and sets the project up, and the manual steps after it. Any page whose goal an agent can carry out says so in one line and links the skill; it does not repeat the prompt.

That splits the writing into two registers. Prose pages (Learn, how-to guides, concepts) read like the feature pages of the website: a claim, a short explanation, one thing to look at. Reference pages and the skill are plain and complete.

## Process

1. Decide whether the docs change at all ("Placing a change").
2. Find the page that owns the fact in the site map. Edit there. Do not add the same detail to a second page.
3. Check the page's kind and follow its rules.
4. Write. Then cut: read each sentence and ask whether a reader with this page's goal would miss it. If not, it moves to reference or goes.
5. Verify ("Accuracy").
6. If the page is new, add it to `docs/nav.yaml` and to the site map. A rename or a new title changes `nav.yaml`, the site map and every link to the page in the same commit.
7. If the fact is one the skill states, change the skill in the same pull request.
8. Put a `Docs:` line in the pull request description: the pages changed, or `none` and the reason.

## Placing a change

| The change | Where it goes |
|---|---|
| A command, flag or help text | The clap help in `crates/cli`. The pages under `docs/reference/cli/` are generated from it; never edit them. |
| A config key, its default or its meaning | The doc comment in `crates/config` (`crates/config/AGENTS.md` has the rules). The website builds the config reference from the schemas. |
| Something a user can now set out to do | A how-to guide: a new one, or a section of the guide with the same goal. Link the example that shows it. |
| A changed behaviour | The sentence that describes it, on the page that owns it. Capture any output it changes again. |
| A limit, a timing, a format, an error message | Reference or a row in `troubleshooting.md`. A concept page may state the scale of a timing it owns, once. |
| Something devlanes leaves out by design | The concept page of that subject, with the reason. |
| A feature devlanes lacks | One entry on `reference/limits.md` with its issue. |
| A change in how devlanes is built or reasons | A concept page. |
| A removed feature | Delete every mention. Do not say it was removed. |
| A bug fix | Nothing, unless a page described the bug. |

When no page fits, add a page; do not stretch one. New content never goes at the end of a page because the end is nearest. A page that would gain a second goal splits in two.

## Page kinds

Every page has one kind, set in its frontmatter as `kind`.

**Overview** (`docs/README.md` only). One paragraph saying what devlanes does, the prompt to paste into an agent, one `devlanes status` block, one diagram with a short caption, and a list of ways in by goal. It names only the words a first-time reader needs at once: project, lane, daemon, proxy.

**Learn** (`docs/learn/`). A short sequence read in order, with previous and next links. It tells how one project, `myapp`, comes to run under devlanes: a service on a hostname, a second worktree, services that find each other, a database per lane, a failure, an agent, another machine. The reader reads and looks; they do not type along. No "open your terminal", no "now run", no "create a branch". Each page shows what the config says, what devlanes does with it and what comes back, and names each new idea once, in a sentence. Where the app itself has to do something, such as listen on the port it is given, the page shows those lines of the app. A page ends by linking the how-to guides that do the same on the reader's own project.

**How-to guide** (`docs/how-to/`). One goal a person has, named in the title with an imperative verb: "Share one Postgres between lanes". It assumes the reader knows devlanes from Learn. A guide the overview lists as a way in may open with one sentence of prerequisites that links the install page and the first Learn page. It opens with one line saying when the guide applies, gives the steps, then the variations as "If ..., ...". It does not teach and does not describe keys one by one; it links the reference for those. It stops when the goal is reached, and may end with a "Related" section of at most four links.

**Concept** (`docs/concepts/`). One topic explained: how it works, the reasons behind it, what was traded for what. Prose and diagrams, no steps. A reader could take it in away from the keyboard. It answers the questions a person has in their first week: what happens when they delete a worktree, close the lid, reboot, or hand the repository to a colleague. A reason on the page is one the code, `CONTRIBUTING.md` or the owner states. Where none is stated, give the fact and its consequence and no motive.

**Reference** (`docs/reference/`, `docs/errors/`). Description only, in the same layout for every entry, complete and without advice. Tables and lists are fine here.

`troubleshooting.md` is one page indexed by the symptom or message a reader sees, each row linking the page that fixes it.

A page that mixes kinds is the main way these docs went wrong before. Explanation inside a how-to guide, steps inside a concept and option lists inside a Learn page each move to the page of their kind, with a link left behind.

## Page shape

These hold for Learn pages, how-to guides and concepts.

- The first paragraph gives the situation and what comes out of it, in plain words, with no inline code. It does not talk about the page: no "this page", no "here you will".
- A section is a heading, one or two short paragraphs and one thing to look at: a terminal block, a config block, a few lines of the app's code or a diagram. On a concept page a section may be prose alone, up to three paragraphs, when there is nothing real to show. On a how-to guide an "If ..." variation may be a short paragraph and a link, and a step with nothing to show joins the step before it.
- The heading states the point once. The sentence under it does not say it again; it gives the reason, the consequence or what to look for in the block. A section whose only sentence repeats its block is cut.
- Commands and their output appear only in `console` blocks, never in a table or a sentence. Config appears only in `yaml devlanes` blocks; to talk about one line, show it as a fragment. Prose refers to both in words: "the start prints one line per service".
- A command or a flag may be named in prose once, in code, when the sentence is about it. Naming it beats talking around it: write `devlanes lane resume`, not "a resume of all lanes". `--no-ui` is explained on `learn/first-service.md` and nowhere else.
- A term is defined before it is used, in one clause, or the page links the glossary at its first use. Unit, task, setup, profile, box and data folder are the ones writers forget.
- At most two inline code spans in a paragraph. A paragraph that needs more belongs in reference.
- Code spans are for what a person types or reads on screen: a command, a key, a hostname, a file name. The names of the project, its services and its branches are plain words: myapp, the API, the main branch.
- A paragraph has at most four sentences, and a sentence carries one idea. No parentheses that list alternatives.
- Lists are for parallel items. No nested lists and no bold inside a list item. Definitions belong in the glossary.
- Headings are sentence case. On Learn and concept pages a heading states the point of its section ("Each lane gets its own ports"). On how-to guides it names the step or the case ("On Linux"). No heading starts with What, Why, Where or How.
- Size: a Learn page about 800 words, a how-to guide about 600, a concept about 1,000. A page over its size usually has a second goal.
- Limits, formats, timings and edge cases do not sit in prose. One sentence may name that a limit exists, with a link. A concept page may give the scale of a timing it owns, once ("frozen after 15 minutes by default"), so the reader knows whether it is seconds or hours. The exact values, thresholds and counts go to reference.
- A fact has one owner. Every other page gets one sentence and a link, and a page does not state the same point in two of its own sections.
- A one-sentence summary of a fact another page owns has to stay true. Check it against that page, and leave the fact out before you blur it.
- Link text is the title of the page it opens, as the site map lists it.

## Voice

- Address the reader as "you". Present tense. No "we", no "let's".
- Lead with the point. No preamble, no announcing what a section will do, no summary at the end of a section or page.
- Plain words. One noun per thing: a service stays a service.
- No em dashes. No unicode arrows or smart quotes.
- No "It's not X, it's Y". No rhetorical questions. No fragments set alone for emphasis. No groups of three for rhythm.
- No filler: "note that", "it's worth noting", "simply", "just", "of course", "as mentioned".
- No selling: seamless, powerful, effortless, robust, magic, instantly, zero-config, "spin up", leverage, unlock, ecosystem.
- Do not count before a list ("three ways", "two reasons").
- No "any", "every", "always", "never" or "nothing" unless it is true without exception. Most such sentences are wrong on one OS or in one case.
- Vary the sentences. A run of short flat statements of the same shape reads as generated, and so does a list of three in every opening.
- State what devlanes does. A feature it lacks is an entry on the limits page, not a caveat on every page. Something it leaves out by design is explained once, on the concept page of that subject.
- Write from the reader's side where you can: "you close the lid", "you delete the branch". A page of rules with a generic subject ("a start runs", "a reload stops") reads like a specification.

## Words and names

| Use | For | Not |
|---|---|---|
| worktree | a Git worktree | checkout, working copy |
| lane | a running copy of the project for one worktree | environment, instance, slot, and "copy" once the word lane is introduced |
| project | what one `devlanes.yaml` describes | repo, workspace, and "app" for what devlanes runs |
| project file | `devlanes.yaml` | config, manifest, and "project file" for `package.json` and the like |
| user config | `~/.devlanes/config.yaml` | settings file, global config |
| local file | `devlanes.local.yaml` | override file |
| unit | a service, task, exec command, setup or teardown entry, or external port | process, job |
| service | a long-running process of a lane | server, app |
| container | something under `infra`; a lane has containers next to its units | infra (in prose), unit |
| unused lane | a lane with no activity, which is throttled, then frozen | idle lane: `idle` is the state of a lane that runs nothing, and of a service that has not run |
| thaw | a frozen lane running again | resume, wake: `devlanes lane resume` brings lanes back after the daemon went away |
| ready check | `ready` | health check, health route, probe |
| daemon | the background process that owns the lanes | server, agent |
| box | a machine whose lanes are mirrored to another | remote, host, server |
| your machine | where the browser runs when a box is involved | client, laptop |
| hostname | `api.myapp1.localhost` | domain, address, name, URL (for the name) |
| start, stop | bringing a lane up or down | spin up, boot, launch, kill |

Say "worktree" for the folder and "lane" for what runs from it.

The Not column is about devlanes' own things. A program such as Vite is still a dev server, and what a CI job clones is still a checkout.

Names in examples:

- The project is `myapp`, with services `api`, `web` and `worker`.
- Lane domains have no hyphen: `myapp.localhost` for lane 0, `myapp1.localhost` for lane 1.
- Branches are `main`, `feat/checkout`, `fix/login` and `agent/perf`. The box is `my-vps`.
- Databases are `myapp_main` and `myapp_lane1`. The folders are `~/dev/myapp` and, for the second worktree, `~/dev/myapp-checkout`.
- Outside hosts end in `example.invalid`. No real names, credentials or private paths.

## Left out of the docs

The docs describe devlanes as it is in the release they ship with.

- No history: no "now", "no longer", "new", "previously", "as of version", and no mention of a feature, command, key or file that was removed or renamed.
- No plans. What devlanes does not do is one entry on `reference/limits.md` with its issue link, and nothing more.
- No internals: crate names, protocol details, database tables, span names. A concept page explains behaviour a user can observe. Which data leaves a machine is such behaviour; the names of messages and fields are not.
- No contributor material. That goes in `CONTRIBUTING.md`.
- No account of how a page came to be, and no notes to other writers. A page is not a changelog.

The one exception to "no versions": when two machines must run releases that match, as a box and the machine connected to it do, say so.

## Examples

The examples repository, `devlanes-run/examples`, holds runnable projects. A how-to guide links the one that shows its goal working, with the example directive and the slug from that repository's `catalog.json`.

Write the page the way it should read first. If no example fits, add one to the examples repository; do not bend a page around an example that almost fits. Never paste an example's whole config into a page: show the lines the page is about.

## Accuracy

A wrong page is worse than a missing one.

- Run every command a page shows, on the release the page is for. Read `devlanes <command> --help` before naming a flag.
- Output in a `console` block comes from a real run of the command the block shows. Shorten it by dropping whole lines. Change nothing else except names, to the ones above, and paths: the user name becomes `you` and the project's folder becomes the one above, in the form the capture's OS prints (`C:\Users\you\dev\myapp` on Windows). Never write output from memory, and never fill a block with lines from another command or another run.
- Capture under the docs' names, so nothing has to be renamed. Where a rename or dropped rows leave a column the command aligned wider or narrower than its values, re-align it with spaces and change nothing else.
- Output in an example's README counts as a capture only when it was run on the release the page is for. Otherwise run the example again.
- A command whose effect the reader has to see is shown with its output. A command may stand alone in a block when its output is beside the point, as with an installer. A command you have no capture for and whose output matters waits for one.
- The command in a block is the one that was run. Do not rewrite it into the form another OS would use. A command that differs by OS is a tab per OS, each with its own capture.
- Note the OS of every capture. Output that differs by OS (ports, paths, resolvers, trust stores) goes in OS tabs, or carries the OS in the block's title, such as `title="on Windows 11"`. Prefer artefacts that read the same everywhere: devlanes' own commands over `curl`.
- Show the command a person types and the output a person sees. A start on a terminal runs inside the TUI, so show starts with `--no-ui`. When an agent captures output, devlanes switches to its terse agent format; capture with `--output human` and `NO_COLOR=1`, and leave those two out of the block. Any other flag that was passed stays in the block, `--output plain` included.
- When you have no real output for something, show the config or a diagram instead. A page without an output block is fine; a page with invented output is not.
- A prompt block asks only for what `skills/devlanes/` teaches an agent to do. Check the skill before you write one.
- A `yaml devlanes` block that is a whole file passes `devlanes config check`. A block that is a fragment uses only keys the schema has; the website build fails on an unknown key. A fragment does not lean on a variable or an entry the page never shows, such as a `lanes.vars` value from an example.
- Check a claim about behaviour against the code or a run, not against the old docs or another page.
- A claim about another tool (Next.js, Vite, Tauri, Wrangler, Tailscale) rests on that tool's own documentation or a run, and names no version ranges. Where the behaviour depends on the version, start the step from the symptom the reader sees.
- A how-to guide whose goal nobody has reached on the release, with an example that is not `ready`, carries `publish: false` until someone has.
- Follow every link you add.
- Read the page once more against "Voice" and "Page shape", and count the code spans in each paragraph.

## Generated pages

- `docs/reference/cli/` comes from the clap definitions through a test in `crates/cli`. Change the help text and regenerate.
- The config reference is not in this repository. The website renders it from `crates/config/schema.json` and `crates/config/user-schema.json`, so its text is the doc comments in `crates/config`. Each comment has to read well alone, with its first sentence a summary.

## The public skill

`skills/devlanes/SKILL.md` is loaded by an agent on any task in a repository that uses devlanes, so every line costs context on every task.

- `SKILL.md` covers daily use: the words (project, lane, unit, box), running this worktree's copy of the project, what a new worktree needs, getting URLs and ports, logs, tests, ad-hoc long-running commands, stopping what the agent started, what to tell the user to open, states, exit codes and errors.
- It does not teach configuration. Setting devlanes up in a repository for the first time, writing or changing `devlanes.yaml`, migrating from another tool, registering the MCP server, setting up a box and fixing hostnames, proxy and lane problems each live in a file under `references/`, named at the top of `SKILL.md` with the situation that calls for it.
- It is the way most people try devlanes: they paste the prompt from the install page and their agent does the rest. `references/setup.md` has to take an agent from a repository without devlanes to a running lane and a URL for the user, and say which steps only the user can do, such as answering the prompt of `devlanes setup`.
- It is exact and terse: imperative sentences, tables, real commands. The prose rules above about inline code and page shape do not apply to it. The rules under "Voice", "Words and names", "Left out of the docs" and "Accuracy" do.
- Every command and flag in it exists in `--help`. It never guesses output.
- An agent cannot open the TUI or the user's browser. The skill tells the agent what to ask the user to run or open.
- It links the public docs by their Markdown twin, `https://devlanes.run/docs/<page>.md`, never by a GitHub path.
- The site map marks the pages whose facts the skill repeats. Change both together.
