---
title: Expressions
description: The syntax of expressions in the project file, the names they read, and the functions they can use.
kind: reference
---

Values in the project file and the local file can hold `${{ ... }}` expressions, which devlanes evaluates for each lane. [Environment and expressions](../concepts/environment.md) explains where their values go.

## Syntax

| Form | Meaning |
|---|---|
| `${{ expression }}` | An expression, in the expression language of MiniJinja templates. |
| `text ${{ a }} text` | Text with expressions in it. The result is text. |
| `${{ '${{' }}` | A literal `${{`. |
| `${{ a \| default('x') }}` | A filter. Filters and tests are MiniJinja's built-in ones, such as `default`, `upper`, `replace`, `join`, `int`, `tojson` and `is defined`. |
| `${{ 'a' if lane.index == 0 else 'b' }}` | A condition. |

A value that is one whole expression keeps its type, so `autostart: ${{ lane.index == 0 }}` is a boolean. Text with an expression in it is text. A name that does not exist is an error; `default` gives it a value.

## Namespaces

| Name | Value |
|---|---|
| `project.name` | `project` from the project file. |
| `project.primary_path` | The path of the primary worktree. |
| `lane.index` | The lane number: `0`, `1`, `2` and so on. |
| `lane.name` | `main` for lane 0, `lane<N>` for lane N. |
| `lane.label` | The branch with each `/` replaced by `-`, such as `feat-checkout`. |
| `lane.path` | The path of the lane's worktree. |
| `lane.data_path` | The [lane data folder](files.md#lane-data-folder). |
| `lane.domain` | The lane domain: `myapp.localhost` in lane 0, `myapp1.localhost` in lane 1. |
| `lane.port` | The first port of the lane's port block. |
| `lane.<var>` | An entry of `lanes.vars`. |
| `self.name` | The name of the service the value belongs to. |
| `self.port` | Its port. |
| `self.ports.<name>` | Its named port `<name>`. |
| `self.url` | `http://127.0.0.1:<port>`. |
| `self.public_url` | `https://` and its first hostname. Undefined for a service without hostnames. |
| `self.hosts` | Its hostnames. |
| `services.<service>.<field>` | The same fields for another service of the lane. |
| `lanes.main.services.<service>.<field>` | The same fields for a service of lane 0. |
| `projects.<project>.services.<service>.<field>` | The same fields for a service of another running project. |
| `infra.<container>.port` | The host port of a container's first published port. |
| `infra.<container>.ports['<port>']` | The host port published for a container port of a Compose service. |
| `infra.<container>.ports.<name>` | The host port of a named port of an `infra.containers` entry. |
| `env.<NAME>` | A variable from the shell that ran the devlanes command, else from the lane's `.env` file. |
| `replica.index`, `replica.count` | The replica's number, from 1, and the service's `replicas`. |

In a task, `self` holds only `name`. `infra.containers`, `infra.compose`, `infra.hosts`, `infra.env` and `infra.external` read only `project`, `lane`, `infra` and `env`.

A name with a dash is written `services['crm-api']` or `services.crm_api`.

## Functions

| Function | Result |
|---|---|
| `domain(name)` | The domain of `name` in this lane, by the rule of `lane.domain`: `pages.localhost` in lane 0, `pages2.localhost` in lane 2. |
