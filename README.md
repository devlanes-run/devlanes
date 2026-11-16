<p align="center">
   <b>Using devlanes?</b> Please consider <a href="https://github.com/sponsors/arthurfiorette" target="_blank">donating</a> to support my open source work ❤️
  <br />
  <sup>
   Help devlanes grow! Star and share this amazing repository with your friends and co-workers!
  </sup>
</p>

<br />

<p align="center" title="devlanes">
  <a href="https://devlanes.run" target="_blank" rel="noopener noreferrer">
    <img src="https://devlanes.run/assets/banner.png" alt="devlanes: Sandboxes for your agents. localhost for you." />
  </a>
</p>

<br />

<p align="center">
  <a title="Apache-2.0 license" target="_blank" href="https://github.com/devlanes-run/devlanes/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/github/license/devlanes-run/devlanes"></a>
  <a title="Latest release" target="_blank" href="https://github.com/devlanes-run/devlanes/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/devlanes-run/devlanes"></a>
  <a title="CI" target="_blank" href="https://github.com/devlanes-run/devlanes/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/devlanes-run/devlanes/ci.yml?branch=main&label=ci"></a>
  <a title="npm package" target="_blank" href="https://www.npmjs.com/package/devlanes"><img alt="Downloads" src="https://img.shields.io/npm/dw/devlanes?style=flat"></a>
  <a title="Last commit" target="_blank" href="https://github.com/devlanes-run/devlanes/commits/main"><img alt="Last commit" src="https://img.shields.io/github/last-commit/devlanes-run/devlanes"></a>
</p>

<br />
<br />

# devlanes

> Sandboxes for your agents. localhost for you.

<br />

devlanes runs your dev stack once per Git worktree, so you and your coding agents can work on different branches at the same time. Each worktree gets a lane with its own ports, HTTPS hostnames and data. Lanes run on your machine or on a remote one, and open in your browser either way.

It runs on macOS, Linux and Windows 11, and it is in beta.

<br />

- [Read the docs.](https://devlanes.run/docs/)
- [Browse the examples.](https://devlanes.run/examples/)

<br />

## Why devlanes

- [A lane per worktree](https://devlanes.run/docs/learn/worktrees/): the main branch at `https://myapp.localhost`, a feature branch at `https://myapp1.localhost`, each on its own ports.
- [HTTPS hostnames](https://devlanes.run/docs/concepts/hostnames-and-https/) with certificates your browser trusts, and no port to remember.
- [A database per lane](https://devlanes.run/docs/learn/databases/): one Postgres container, and a database in it for each lane.
- [Coding agents](https://devlanes.run/docs/how-to/agents/set-up/) start, test and stop their own lane through a skill or an MCP server, and leave yours alone.
- [Remote boxes](https://devlanes.run/docs/learn/remote/): run the lanes on a VPS, a container or a codespace, and open them under the same hostnames.
- [One daemon](https://devlanes.run/docs/concepts/daemon/) runs the processes, so they keep running after you close the terminal. A CLI and a TUI talk to it.

## Get started

Paste this into your coding agent:

```text
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine if it is missing, set it up for this repository, start it and give me the URL of each service.
```

Or install it yourself, on macOS and Linux:

```sh
curl -fsSL https://devlanes.run/install.sh | sh
```

On Windows 11:

```powershell
irm https://devlanes.run/install.ps1 | iex
```

Homebrew, npm, uv and cargo work too. [Install devlanes](https://devlanes.run/docs/install/) has them, along with the steps that follow the install.

## A project in one file

```yaml
# devlanes.yaml
project: myapp

services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
    hosts: "${{ lane.domain }}"
```

```sh
devlanes start
```

In the main worktree this serves `https://myapp.localhost` and `https://api.myapp.localhost`. In a second worktree the same file serves `https://myapp1.localhost` and `https://api.myapp1.localhost`, on other ports.

## Documentation

The [docs](https://devlanes.run/docs/) start with a [tour](https://devlanes.run/docs/learn/first-service/) that follows one project from a single service to lanes on another machine. `devlanes --help` lists the commands.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers building, testing and the design notes, and [AGENTS.md](AGENTS.md) has the conventions for people and agents working on the code.

mprocs and dekit informed the terminal interaction. pitchfork, process-compose, devenv, Coasts, portless, kache, overmind and Conductor informed the process and worktree workflows.

## License

Licensed under the **Apache-2.0 License**. See [`LICENSE`](LICENSE) for more information.
