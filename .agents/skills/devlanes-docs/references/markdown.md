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
- `kind`: `learn`, `how-to`, `concept` or `reference`.
- `publish: false` keeps a file off the website. `docs/AGENTS.md` has it.

## Sidebar

`docs/nav.yaml` lists every page in sidebar order. A page missing from it lands in the last group with a build warning. Add a new page to `nav.yaml` and to the site map in the same change.

## Links

- Link another docs page by its relative path with the `.md` extension: `[Idle lanes](../concepts/idle-lanes.md)`. The website turns it into the route.
- Link a heading with `#` and its slug: `../concepts/ports.md#named-ports`.
- Link text names the page or the thing. Never "here" or "this page".
- The config reference is generated on the website, so link it by route: `/docs/reference/config/project/services/`.

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
- `title` names where the command runs when that matters: a folder, "on the box", "on your machine".
- One command with its output per block where possible. A block of several commands with no output is a script.
- Output is real ("Accuracy" in `SKILL.md`).

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

Sync keys: `os` (macOS, Linux, Windows), `install` (the install methods), `agent` (agent hosts). Use tabs only where the content differs by that key. Tabs with the same key switch together.

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

An SVG file next to the page. It uses `currentColor` and the variables `--dl-diagram-line`, `--dl-diagram-fill` and `--dl-diagram-accent`, so it follows the theme. Alt text says what the diagram shows.

## Asides

```markdown
:::caution
`devlanes lane clean` deletes the lane's data.
:::
```

`:::note`, `:::tip`, `:::caution` and `:::danger`. Use one only for something a reader loses data or time by missing. A page with more than one aside has a structure problem.
