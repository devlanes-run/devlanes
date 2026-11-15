---
title: Expressions
description: The syntax of expressions in the project file, the names they read, and the functions, filters and tests they can use.
kind: reference
---

Values in the project file and the local file can hold `${{ ... }}` expressions, which devlanes evaluates for each lane. [Environment and expressions](../concepts/environment.md) explains where their values go.

## Syntax

| Form | Meaning |
|---|---|
| `${{ expression }}` | An expression. The language is that of MiniJinja templates, restricted to expressions. |
| `text ${{ a }} text ${{ b }}` | Text with expressions in it. The result is text. |
| `${{ '${{' }}` | A literal `${{`. |
| `{{ ... }}`, `{% ... %}`, `{# ... #}` | Plain text. Statements and template comments are not part of the syntax. |

Inside an expression:

| Form | Example |
|---|---|
| Attribute | `lane.domain` |
| Subscript | `infra.postgres.ports['5432']`, `services['crm-api'].url` |
| Arithmetic | `lane.port + 5`, `lane.index * 10` |
| Comparison and logic | `lane.index == 0`, `not a`, `a and b`, `a or b` |
| Membership | `'API_KEY' in env` |
| Concatenation | `'acme_' ~ lane.name` |
| Condition | `'acme' if lane.index == 0 else 'acme_' ~ lane.name` |
| Filter | `env.MODE \| default('dev') \| upper` |
| Test | `env.MODE is defined` |
| Literals | `'text'`, `"text"`, `42`, `1.5`, `true`, `false`, `none`, `[1, 2]`, `{'a': 1}` |

## Namespaces

### project

| Field | Type | Value |
|---|---|---|
| `project.name` | text | `project` from the project file. |
| `project.primary_path` | text | The path of the primary worktree, the worktree of lane 0. |

### lane

| Field | Type | Value |
|---|---|---|
| `lane.index` | number | The lane number: `0` for the primary worktree, then `1`, `2` and so on. |
| `lane.name` | text | `main` for lane 0, `lane<N>` for lane N. A lane added with `devlanes lane new --name` also has `lane<N>` here. |
| `lane.label` | text | The branch with each `/` replaced by `-`, such as `feat-checkout`. On a detached HEAD, the lane name. |
| `lane.path` | text | The path of the lane's worktree. |
| `lane.data_path` | text | The [lane data folder](files.md#lane-data-folder). On Windows it is written with `/`. |
| `lane.domain` | text | The lane domain: `myapp.localhost` in lane 0, `myapp1.localhost` in lane 1. A `lanes.vars` entry named `domain` replaces it. |
| `lane.port` | number | The first port of the lane's port block. A `lanes.vars` entry named `port` sets where the block starts. |
| `lane.<var>` | any | Each entry of `lanes.vars`, or of the lane's row of `lanes.matrix`. A dash in the name can be written as `_`. |

### self

The service or unit whose field is being evaluated.

| Field | Type | Value |
|---|---|---|
| `self.name` | text | The service name from the project file, the same for every replica. |
| `self.port` | number | The service's port. |
| `self.ports.<name>` | number | A named port of the service. Naming one in the service's own fields gives the service that port. |
| `self.url` | text | `http://127.0.0.1:<port>`. |
| `self.public_url` | text | `https://` and the first hostname without a wildcard, with the proxy's HTTPS port unless it is 443. Undefined for a service without hostnames. |
| `self.hosts` | list | The service's hostnames. |

In a task and in an `infra.setup` or `infra.teardown` entry, `self` holds `name` only.

### services

| Field | Type | Value |
|---|---|---|
| `services.<name>.name` | text | As `self.name`, for the service `<name>` of the same lane. |
| `services.<name>.port` | number | As `self.port`. |
| `services.<name>.ports.<port>` | number | As `self.ports.<name>`. Naming a port here does not give the service that port. |
| `services.<name>.url` | text | As `self.url`. |
| `services.<name>.public_url` | text | As `self.public_url`. |
| `services.<name>.hosts` | list | As `self.hosts`. |

A service with replicas reads as its first replica here.

### lanes

| Field | Type | Value |
|---|---|---|
| `lanes.main.services.<name>.<field>` | as `services` | The service `<name>` of lane 0 of the same project, with the fields of `services`. In lane 0 it is the lane's own service. |

### projects

| Field | Type | Value |
|---|---|---|
| `projects.<project>.services.<name>.<field>` | as `services` | The service `<name>` of another project, with the fields of `services`. |

The lane of the other project is, among its lanes that run at least one service: the lane on the same branch, else the lane with the same `lane.name`, else its lane 0. A project with no such lane reads as undefined, so the expression needs `default`. The value is fixed when the unit starts.

### infra

| Field | Type | Value |
|---|---|---|
| `infra.<container>.port` | number | The host port of the container's first published port. |
| `infra.<container>.ports['<container port>']` | number | For a Compose service: the host port published for that container port. |
| `infra.<container>.ports.<name>` | number | For a container under `infra.containers`: the host port of its named port. |

`<container>` is a Compose service or an `infra.containers` name. A container that is not running reads as undefined.

### env

| Field | Type | Value |
|---|---|---|
| `env.<NAME>` | text | The variable from the environment of the shell that ran the devlanes command, else from the `.env` file next to the project file. |

The `.env` file is the one in the lane's worktree, or the primary worktree's when the lane's worktree has none. On Windows, `env` names ignore case: `env.Path` reads `PATH`.

### replica

| Field | Type | Value |
|---|---|---|
| `replica.index` | number | The replica's number, from 1. |
| `replica.count` | number | The service's `replicas`. |

Outside a service, `replica` is empty.

## Names with dashes

A dash in an expression is subtraction, so `services.crm-api.url` reads as `services.crm - api.url`. A name with a dash is written in one of two forms:

| Form | Example |
|---|---|
| Subscript | `services['crm-api'].url` |
| Underscore | `services.crm_api.url` |

The underscore form exists for every key with dashes in every namespace, unless two keys give the same underscore name or a key with that name already exists.

## Functions

| Function | Result |
|---|---|
| `domain(name)` | The domain of `name` in this lane, by the rule of `lane.domain`: `name.localhost` in lane 0, `name<N>.localhost` in lane N, with the index after the last label of `name`. `name` is lowercased. It fails for an empty name, a wildcard, or a label that breaks hostname rules. Under the `resolver` hostname strategy the suffix is the configured TLD instead of `localhost`. It works in `lanes.vars`. |
| `range(stop)`, `range(start, stop, step)` | A list of numbers. |
| `dict(key=value, ...)` | A mapping. |
| `namespace(...)` | A mutable object, as in MiniJinja. |
| `debug()` | A text dump of the values in scope. |

## Filters

Filters are written after `|`: `${{ env.MODE | default('dev') }}`.

| Filter | Result |
|---|---|
| `abs` | Absolute value. |
| `attr(name)` | The attribute `name`. |
| `batch(count, fill)` | The list in chunks of `count`. |
| `bool` | The value as `true` or `false`. |
| `capitalize` | First letter uppercase, the rest lowercase. |
| `chain(other, ...)` | The lists joined into one. |
| `count`, `length` | Number of items or characters. |
| `default(value, boolean)`, `d` | `value` when the input is undefined, or also when it is false, empty or none with `boolean` set to `true`. |
| `dictsort` | The mapping's pairs sorted by key. |
| `escape`, `e` | Text with HTML characters escaped. |
| `first` | The first item. |
| `float` | The value as a decimal number. |
| `format(...)` | printf-style formatting of the input. |
| `groupby(attribute)` | Items grouped by an attribute. |
| `indent(width)` | Lines after the first indented by `width`. |
| `int` | The value as a whole number. |
| `items` | The mapping's key and value pairs. |
| `join(separator)` | The items as text, joined with `separator`. |
| `last` | The last item. |
| `lines` | The text split into lines. |
| `list` | The value as a list. |
| `lower` | Lowercase. |
| `map(filter)`, `map(attribute=name)` | A filter or an attribute applied to each item. |
| `max`, `min` | The largest or smallest item. |
| `pprint` | A readable dump of the value. |
| `reject(test)`, `select(test)` | The items that fail or pass a test. |
| `rejectattr(attribute, test)`, `selectattr(attribute, test)` | The items whose attribute fails or passes a test. |
| `replace(old, new)` | Text with each `old` replaced by `new`. |
| `reverse` | The items or characters in reverse order. |
| `round(precision)` | The number rounded. |
| `safe` | Text marked as not needing escaping. |
| `slice(count)` | The list split into `count` parts. |
| `sort` | The items sorted. |
| `split(separator)` | The text split into a list, on whitespace without `separator`. |
| `string` | The value as text. |
| `sum` | The sum of the items. |
| `title` | Each word capitalized. |
| `tojson` | The value as JSON text. |
| `trim(characters)` | Text without leading and trailing whitespace, or without `characters`. |
| `unique` | The items without duplicates. |
| `upper` | Uppercase. |
| `zip(other, ...)` | The lists paired item by item. |

## Tests

Tests are written after `is`: `${{ env.MODE is defined }}`.

| Test | True when the value |
|---|---|
| `boolean` | is `true` or `false`. |
| `defined`, `undefined` | is defined, or undefined. |
| `divisibleby(n)` | is a multiple of `n`. |
| `endingwith(text)`, `startingwith(text)` | ends or starts with `text`. |
| `eq(other)`, `equalto`, `==` | equals `other`. |
| `ne(other)`, `!=` | differs from `other`. |
| `lt(other)`, `lessthan`, `<` | is less than `other`. |
| `le(other)`, `<=` | is less than or equal to `other`. |
| `gt(other)`, `greaterthan`, `>` | is greater than `other`. |
| `ge(other)`, `>=` | is greater than or equal to `other`. |
| `escaped`, `safe` | is marked safe. |
| `even`, `odd` | is an even or odd number. |
| `false`, `true` | is `false`, or `true`. |
| `filter`, `test` | names a filter, or a test. |
| `float` | is a decimal number. |
| `in(container)` | is in `container`. |
| `integer`, `int` | is a whole number. |
| `iterable` | can be iterated. |
| `lower`, `upper` | is all lowercase, or all uppercase. |
| `mapping` | is a mapping. |
| `none` | is none. |
| `number` | is a number. |
| `sameas(other)` | is the same object as `other`. |
| `sequence` | is a list. |
| `string` | is text. |

## Namespaces by field

| Field of the project file | Namespaces it reads |
|---|---|
| A service's fields: `run`, `run_script`, `cwd`, `env`, `hosts`, `ready`, `stop`, `pty`, `autostart`, `hidden`, `keep_awake`, `sync` and preset options | All. `self` is the service, `replica` its replica. |
| A task's fields | All. `self` holds `name` only, `replica` is empty. |
| An `infra.setup` or `infra.teardown` entry | All. `self` holds `name` only, `replica` is empty. |
| `global_env` | All. An entry that reads `self` or `replica` is evaluated for each unit with that unit's values. |
| `profiles.<name>.env` | As `global_env`. |
| `profiles.<name>.infra` | All except `self` and `replica`. |
| `lanes.vars`, `lanes.matrix` | `project`, `lane` and `env`. `lane` holds its fixed fields and the other vars. `lanes.vars.port` is evaluated before the port block exists, so it cannot read `lane.port`. |
| `infra.containers`, `infra.compose`, `infra.hosts`, `infra.env`, `infra.external` | `project`, `lane`, `infra` and `env`. |

A service's `hosts` and `env.PORT` may read other services' `port`, `url` and `public_url`. A chain of such references that comes back to where it started is a config error. `env.PORT` cannot read `self.port`, `self.url` or `self.public_url`.

## Types

| Value | Result |
|---|---|
| One whole expression, such as `${{ lane.index == 0 }}` | The expression's own type: a number, `true` or `false`, text, a list or a mapping. It has to fit the field. |
| Text with expressions in it, such as `api.${{ lane.domain }}` | Text. |
| An item of a `run` list, or shell text in `run` | Text. A number or a boolean becomes its text form. A list, a mapping or none is an error. |
| An `env` value | Text. A number or a boolean becomes its text form, such as `20000` or `true`. |

```yaml devlanes
services:
  docs:
    run: [rspress, dev, --port, "${{ self.port }}"]
    autostart: ${{ lane.index == 0 }}
```

A result that does not fit its field fails with `expression result has the wrong type`.

## Undefined names

A name that does not exist is an error. `default` turns it into a value.

| Case | Result |
|---|---|
| An unknown namespace, such as `${{ lan.index }}` | `devlanes config check` and loading the project file fail with `unknown expression context`, naming the closest namespace. |
| An unknown field of `project`, `lane`, `self`, `services`, `lanes`, `projects.<project>`, `replica` or `infra.<container>` | Both fail with `is not defined`, naming the closest field. |
| A service, container or Compose service that the project does not have | Both fail, naming the closest name. |
| A missing `env` variable, a missing `projects` project, a stopped container, or `public_url` of a service without hostnames | The unit's evaluation fails at start with `expression failed`. |
| Any of these with `\| default(value)` | `value`. |

```yaml devlanes
global_env:
  SENTRY_DSN: ${{ env.SENTRY_DSN | default('') }}
```

`devlanes config check` evaluates the expressions that read no namespace. The others are evaluated when a lane resolves the unit, and their errors show on the unit.
