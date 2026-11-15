# Markdown the website renders

Pages are `.md` files with the directives below. No MDX, no HTML, no components.

## Frontmatter

```yaml
---
title: Share one Postgres between lanes
description: Run one Postgres container for every lane and give each lane its own database in it.
kind: how-to
---
```

- `title`: sentence case. The page has no `#` heading; the website renders the title.
- `description`: one sentence, shown in search results and in `llms.txt`. Plain words, no inline code.
- `kind`: `overview` (the docs home only), `learn`, `how-to`, `concept` or `reference`.
- `publish: false` keeps a file off the website. `docs/AGENTS.md` has it.

## File names

Lowercase words joined by hyphens. No page is named `agents.md` or `claude.md`: on Windows and macOS an agent's harness reads such a file as its instructions, whatever the letter case.

## Sidebar

`docs/nav.yaml` lists every page in sidebar order. A page missing from it lands in the last group with a build warning. Add a new page to `nav.yaml` and to the site map in the same change.

## Links

- Link another docs page by its relative path with the `.md` extension: `[Throttling and freezing](../concepts/freezing.md)`. The website turns it into the route.
- Link a heading with `#` and its slug: `../concepts/ports.md#named-ports`.
- Link text names the page or the thing. Never "here" or "this page".
- The config reference is generated on the website, so link it by route. A project file key lives at `/docs/reference/config/project/<top-level key>/`, such as `/docs/reference/config/project/services/` or `/docs/reference/config/project/infra/`, and a user config key at `/docs/reference/config/user/<top-level key>/`. The link text is the full dotted key in code, never its last part alone: [`services.<name>.ready`](/docs/reference/config/project/services/).
- A proxy error page is linked by its own title, such as [DL201 Service not running](../errors/service-idle.md).

## Terminal blocks

````markdown
```console title="on the box"
$ devlanes daemon pair
pairing code: K7QM-3XWD
it works once and expires in 10 minutes
```
````

- A line that starts with `$ ` is a command. Every other line is output. The copy button takes the commands only.
- `PS> ` starts a PowerShell command.
- `title` names where the command runs when that matters: a folder, "on the box", "on your machine", or the OS when the output differs by OS.
- One command with its output per block where possible. A block of several commands with no output is a script.
- Output that contains a fenced block of its own, such as an error page in Markdown, goes in a block opened and closed with four backticks.
- Output is real ("Accuracy" in `SKILL.md`).

## Prompt blocks

````markdown
```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then set up devlanes for this repository and give me the URL of each service.
```
````

Text for the reader to paste into a coding agent. It renders as a card with a copy button. Write it as one instruction a person would give, in plain sentences. Use it on the overview and the install page, and on a how-to guide only when the whole guide is something an agent does.

## Config blocks

````markdown
```yaml devlanes
services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
```
````

- `yaml devlanes` is a project file or a fragment of one. `yaml devlanes-user` is the user config.
- A first-line comment with a file name sets the tab title: `# devlanes.local.yaml`.
- Each key shows its description on hover and links to the reference. An unknown key fails the website build.
- Plain `yaml` is for other files: Compose files, CI workflows.
- Comments in a config block say why a line is there. They do not repeat the hover.

## Tabs

```markdown
::::tabs{sync="os"}
:::tab[macOS]
...
:::
:::tab[Linux]
...
:::
:::tab[Windows]
...
:::
::::
```

Sync keys: `os` (macOS, Linux, Windows), `install` (the install methods), `shell` (bash, zsh, fish, PowerShell), `agent` (agent hosts). Use tabs only where the content differs by that key. Two systems that share the content share a tab: `:::tab[macOS and Linux]`. Tabs with the same key switch together. Tabs by anything else, such as a framework or a datastore, are `::::tabs` with no `sync`, and switch alone. A page with several tab groups by the same thing gives them a key of its own so they switch together: `sync="runtime"` on the runtimes guide.

## Steps

```markdown
:::steps
1. Install devlanes on the box.
2. Run `devlanes daemon pair` there.
3. Connect from your machine.
:::
```

For a short run of actions inside a how-to guide. Longer procedures are sections.

## Examples

```markdown
::example{slug="infra-postgres-shared"}
```

The slug is a `slug` from `catalog.json` in the examples repository whose `status` is `ready`. One card per page, placed after the opening line of a how-to guide.

## Diagrams

```markdown
![A box and your machine, joined by one SSH session](remote-mirroring.svg)
```

An SVG file next to the page. The website puts it inline, so it can follow the theme: its only colours are `currentColor` and the variables `--dl-diagram-line`, `--dl-diagram-fill` and `--dl-diagram-accent`, with `currentColor` or `transparent` as the fallback and no colour written as a hex value. The accent is for one highlighted path, and a diagram without one leaves it out. A number in a diagram that is a default says so. Give it a `<title>` that matches the alt text. The alt text says what the diagram shows, and the diagram contains every word the alt text names.

## Asides

```markdown
:::caution
`devlanes lane clean` deletes the lane's data.
:::
```

`:::note`, `:::tip`, `:::caution` and `:::danger`. Use one only for something a reader loses data or time by missing. A page with more than one aside has a structure problem.
