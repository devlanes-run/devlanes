#!/usr/bin/env python3
"""Prints which CI jobs and test packages a change needs.

usage: affected.py [BASE [HEAD]]

Compares HEAD (default: the checked-out commit) with BASE, a commit whose checks all
passed, and prints `key=value` lines for $GITHUB_OUTPUT. Without BASE everything runs. A changed file inside a
workspace package selects that package's tests and the tests of every workspace
package that depends on it, through any kind of dependency; files elsewhere follow
the rules below, and a file no rule knows runs everything. Needs git and cargo;
standard library only.

  lint       lint job: true when a package's own files changed.
  rust       rust job: true when any package's tests are selected.
  tests      nextest filterset of the selected packages; empty runs all of them.
  e2e        e2e and ssh jobs, which drive the devlanes binary.
  web        web job (devlanes-tui-web for wasm32 and its bundle).
  packaging  packaging and packaging-windows jobs.
  mcp_ui     mcp-ui job (the Node build of the MCP Apps card).
  next       on main, the rolling `next` prerelease.
"""

import json
import subprocess
import sys
from fnmatch import fnmatch
from pathlib import PurePosixPath

# CI itself: every job runs.
EVERYTHING = (".github/workflows/ci.yml", ".github/actions/*", ".github/scripts/*")

# Changes every crate's build or how its tests run.
ALL_RUST = (
    "Cargo.lock",
    "*Cargo.toml",
    "rust-toolchain.toml",
    "rustfmt.toml",
    ".cargo/*",
    ".config/*",
    ".gitattributes",
    # Read by the config, resolve and e2e tests.
    "fixtures/*",
)

# Files outside a package that its tests read at run time. Only that package's own
# tests read them, so the packages that depend on it are not selected.
READ_BY = {
    "docs/*": ("devlanes-cli", "devlanes-pages"),
    "skills/*": ("devlanes-cli",),
    ".sampo/changesets/*": ("devlanes-update",),
    # devlanes-cli checks its error codes against the daemon's source.
    "crates/daemon/src/*": ("devlanes-cli",),
    # devlanes-mcp-ui checks its tokens against the pages stylesheet.
    "crates/pages/styles/*": ("devlanes-mcp-ui",),
}

PACKAGING = (
    "packaging/*",
    "install.sh",
    "install.ps1",
    # The npm and PyPI READMEs are built from it.
    "README.md",
    # shellcheck, actionlint and PSScriptAnalyzer.
    "*.sh",
    "*.ps1",
    ".github/workflows/*",
    ".github/actionlint.yaml",
)

WEB = ("packaging/scripts/tui-web.sh",)
MCP_UI = ("crates/mcp-ui/ui/*",)

# What the `next` prerelease ships besides the binaries.
NEXT = ("packaging/*", "install.sh", "install.ps1")

# Nothing in CI reads these.
IGNORED = (
    "AGENTS.md",
    "CONTRIBUTING.md",
    "LICENSE",
    ".gitignore",
    ".agents/*",
    ".github/ISSUE_TEMPLATE/*",
    ".github/dependabot.yml",
    ".sampo/config.toml",
)


def matches(path, patterns):
    """Whether `path` matches one of the glob `patterns`."""
    return any(fnmatch(path, pattern) for pattern in patterns)


def changed_files(base, head):
    """Paths that differ between the commits `base` and `head`, relative to the repository root."""
    out = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", base, head],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in out.splitlines() if line]


def workspace():
    """Each package's directory relative to the root, and the packages depending on each one."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    metadata = json.loads(out)
    root = PurePosixPath(metadata["workspace_root"].replace("\\", "/"))
    dirs = {}
    for package in metadata["packages"]:
        manifest = PurePosixPath(package["manifest_path"].replace("\\", "/"))
        dirs[package["name"]] = manifest.parent.relative_to(root).as_posix()
    by_dir = {dir: name for name, dir in dirs.items()}
    dependents = {name: set() for name in dirs}
    for package in metadata["packages"]:
        for dependency in package["dependencies"]:
            path = dependency.get("path")
            if path:
                target = PurePosixPath(path.replace("\\", "/")).relative_to(root).as_posix()
                if target in by_dir:
                    dependents[by_dir[target]].add(package["name"])
    return dirs, dependents


def owner(path, dirs):
    """The package whose directory holds `path`, the innermost one if they nest."""
    best = None
    for name, dir in dirs.items():
        if path.startswith(dir + "/") and (best is None or len(dir) > len(dirs[best])):
            best = name
    return best


def closure(packages, dependents):
    """`packages` and every package that depends on one of them, directly or not."""
    seen = set(packages)
    stack = list(packages)
    while stack:
        for dependent in dependents[stack.pop()]:
            if dependent not in seen:
                seen.add(dependent)
                stack.append(dependent)
    return seen


def plan(files, dirs, dependents):
    """The outputs for a change to `files`."""
    everything = rust_all = packaging = web = mcp_ui = next_assets = False
    changed = set()
    read = set()
    for path in files:
        if matches(path, EVERYTHING):
            everything = True
            continue
        known = False
        if matches(path, ALL_RUST):
            rust_all = known = True
        for pattern, packages in READ_BY.items():
            if fnmatch(path, pattern):
                read.update(packages)
                known = True
        if matches(path, PACKAGING):
            packaging = known = True
        if matches(path, WEB):
            web = known = True
        if matches(path, MCP_UI):
            mcp_ui = True
        if matches(path, NEXT):
            next_assets = True
        package = owner(path, dirs)
        if package:
            changed.add(package)
            known = True
        if matches(path, IGNORED):
            known = True
        if not known:
            print(f"::notice::{path} matches no rule in .github/scripts/affected.py, so everything runs", file=sys.stderr)
            everything = True
    rust_all = rust_all or everything
    selected = closure(changed, dependents) | read
    binary = rust_all or bool(selected & {"devlanes", "devlanes-e2e"})
    return {
        # Files that tests only read leave clippy and rustfmt nothing new to check.
        "lint": rust_all or bool(changed),
        "rust": rust_all or bool(selected),
        "tests": "" if rust_all else " | ".join(f"package(={name})" for name in sorted(selected)),
        "e2e": binary,
        "web": rust_all or web or "devlanes-tui-web" in selected,
        "packaging": everything or packaging,
        "mcp_ui": everything or mcp_ui,
        "next": binary or next_assets or "devlanes-hosts-helper" in selected,
    }, sorted(selected)


def main(args):
    """Parses the command line and prints the outputs."""
    if len(args) > 2 or any(arg.startswith("-") for arg in args):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        sys.exit(2)
    if not args:
        print("No base commit: everything runs.", file=sys.stderr)
        outputs = {key: True for key in ("lint", "rust", "e2e", "web", "packaging", "mcp_ui", "next")}
        outputs["tests"] = ""
    else:
        files = changed_files(args[0], args[1] if len(args) > 1 else "HEAD")
        dirs, dependents = workspace()
        outputs, selected = plan(files, dirs, dependents)
        print(f"{len(files)} files changed since {args[0]}:", file=sys.stderr)
        for path in files:
            print(f"  {path}", file=sys.stderr)
        print(f"Selected packages: {' '.join(selected) or 'none'}", file=sys.stderr)
    for key, value in outputs.items():
        if isinstance(value, bool):
            value = "true" if value else "false"
        print(f"{key}={value}")


if __name__ == "__main__":
    main(sys.argv[1:])
