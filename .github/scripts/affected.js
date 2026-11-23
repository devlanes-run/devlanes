// Picks the CI jobs and test packages a change needs, for ci.yml's `changes` job.
//
// The change is the diff from a commit whose CI passed: a pull request's base, or on main
// the last commit whose CI run passed. A changed file selects the workspace package that
// holds it and every package that depends on it. A Cargo.lock change selects the packages
// whose locked dependencies changed. Files outside packages select nothing, except those
// listed in Cargo.toml manifests:
// - the root Cargo.toml and its `[workspace.metadata.ci] inputs` run everything;
// - a package's `[package.metadata.ci] build-inputs`, files its build reads from outside
//   its directory, count as its own files;
// - a package's `[package.metadata.ci] inputs`, files only its tests read, select that
//   package alone.
// Paths are relative to the repository root; one ending in `/` covers a directory.

const { execFileSync } = require('node:child_process');

module.exports = async ({ github, context, core }) => {
  const { owner, repo } = context.repo;
  const head = context.sha;

  const base = await baseCommit();
  if (!base) return everything('there is no commit to compare with');

  const workspace = loadWorkspace();
  const result = await select(base);
  if (result === 'all') return everything(`a change since ${base} affects every package`);

  const changed = [...result.changed].sort();
  const affected = [...result.affected].sort();
  core.info(`Compared with ${base}`);
  core.info(`Changed packages: ${changed.join(' ') || 'none'}`);
  core.info(`Packages whose tests run: ${affected.join(' ') || 'none'}`);
  const all = affected.length === workspace.dirs.size;
  const outputs = {
    lint: changed.length > 0,
    rust: affected.length > 0,
    tests: all ? '' : affected.map((name) => `package(=${name})`).join(' | '),
    e2e: result.affected.has('devlanes') || result.affected.has('devlanes-e2e'),
    web: result.affected.has('devlanes-tui-web'),
    mcp_ui: result.changed.has('devlanes-mcp-ui'),
    next: await nextNeeded(),
  };
  for (const [key, value] of Object.entries(outputs)) core.setOutput(key, String(value));

  /** Sets the outputs that run every job, and says why. */
  function everything(reason) {
    core.notice(`Running everything: ${reason}.`);
    core.setOutput('tests', '');
    for (const key of ['lint', 'rust', 'e2e', 'web', 'mcp_ui', 'next']) core.setOutput(key, 'true');
  }

  /** The commit this change is compared with, or null to run everything. */
  async function baseCommit() {
    // HEAD is the pull request's merge commit, whose first parent is this base.
    if (context.eventName === 'pull_request') return context.payload.pull_request.base.sha;
    if (context.eventName !== 'push') return null;
    const { data } = await github.rest.actions.listWorkflowRuns({
      owner,
      repo,
      workflow_id: 'ci.yml',
      branch: context.ref.replace('refs/heads/', ''),
      event: 'push',
      status: 'success',
      per_page: 1,
    });
    return data.workflow_runs[0]?.head_sha ?? null;
  }

  /**
   * On a push, whether the `next` prerelease ships anything that changed since the commit
   * it was built from. That can be older than base when the commits since changed nothing
   * it ships.
   */
  async function nextNeeded() {
    if (context.eventName !== 'push') return false;
    let tag;
    try {
      ({ data: tag } = await github.rest.git.getRef({ owner, repo, ref: 'tags/next' }));
    } catch {
      return true;
    }
    const since = await select(tag.object.sha, ['packaging/', 'install.sh', 'install.ps1']);
    if (since === 'all' || since.assets) return true;
    return since.affected.has('devlanes') || since.affected.has('devlanes-hosts-helper');
  }

  /**
   * The packages changed and affected between `from` and HEAD, or 'all'. `assets` says
   * whether a file in `assetPaths` changed.
   */
  async function select(from, assetPaths = []) {
    const files = await changedFiles(from);
    if (!files) return 'all';
    const changed = new Set();
    const read = new Set();
    let assets = false;
    for (const file of files) {
      if (file === 'Cargo.toml' || matches(file, workspace.inputs)) return 'all';
      if (matches(file, assetPaths)) assets = true;
      const pkg = workspace.packageOf(file);
      if (pkg) changed.add(pkg);
      for (const [name, { inputs = [], 'build-inputs': buildInputs = [] }] of workspace.declared) {
        if (matches(file, buildInputs)) changed.add(name);
        if (matches(file, inputs)) read.add(name);
      }
    }
    if (files.includes('Cargo.lock')) {
      for (const name of await lockChanges(from)) changed.add(name);
    }
    const affected = closure(changed, workspace.dependents);
    for (const name of read) affected.add(name);
    return { changed, affected, assets };
  }

  /** Paths that differ between `from` and HEAD, or null when there are too many to list. */
  async function changedFiles(from) {
    const files = [];
    for (let page = 1; ; page++) {
      const { data } = await github.rest.repos.compareCommitsWithBasehead({
        owner,
        repo,
        basehead: `${from}...${head}`,
        per_page: 100,
        page,
      });
      const listed = data.files ?? [];
      for (const file of listed) {
        files.push(file.filename);
        if (file.previous_filename) files.push(file.previous_filename);
      }
      // The compare API lists at most 3000 files.
      if (listed.length < 100) return files.length < 3000 ? files : null;
    }
  }

  /** Workspace packages whose locked dependencies differ between `from` and HEAD. */
  async function lockChanges(from) {
    const [before, after] = await Promise.all([lockfile(from), lockfile(head)]);
    const dirty = new Set();
    const lockers = new Map();
    for (const [key, entry] of after) {
      if (before.get(key)?.text !== entry.text) dirty.add(entry.name);
      for (const dependency of entry.dependencies) {
        if (!lockers.has(dependency)) lockers.set(dependency, new Set());
        lockers.get(dependency).add(entry.name);
      }
    }
    return [...closure(dirty, lockers)].filter((name) => workspace.dirs.has(name));
  }

  /** Cargo.lock at `ref`, as entries keyed by name, version and source. */
  async function lockfile(ref) {
    const { data } = await github.rest.repos.getContent({
      owner,
      repo,
      path: 'Cargo.lock',
      ref,
      mediaType: { format: 'raw' },
    });
    const entries = new Map();
    for (const text of String(data).split('\n[[package]]\n').slice(1)) {
      const field = (key) => text.match(new RegExp(`^${key} = "([^"]*)"`, 'm'))?.[1] ?? '';
      const list = text.match(/^dependencies = \[([^\]]*)\]/m)?.[1] ?? '';
      // A dependency is written `name`, `name version` or `name version (source)`.
      const dependencies = [...list.matchAll(/"([^" ]+)[^"]*"/g)].map((match) => match[1]);
      const name = field('name');
      entries.set(`${name} ${field('version')} ${field('source')}`, { name, text, dependencies });
    }
    return entries;
  }
};

/** The workspace's package directories, the packages that depend on each, and the inputs. */
function loadWorkspace() {
  // Only reads manifests, so the runner's stable cargo does, without kache.
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      env: { ...process.env, RUSTUP_TOOLCHAIN: 'stable', CARGO_BUILD_RUSTC_WRAPPER: '' },
    }),
  );
  const root = `${metadata.workspace_root.replaceAll('\\', '/')}/`;
  const relative = (path) => path.replaceAll('\\', '/').replace(root, '');
  const dirs = new Map(
    metadata.packages.map((pkg) => [pkg.name, relative(pkg.manifest_path).replace(/\/Cargo\.toml$/, '')]),
  );
  const byDir = new Map([...dirs].map(([name, dir]) => [dir, name]));
  const dependents = new Map();
  const declared = new Map();
  for (const pkg of metadata.packages) {
    for (const dependency of pkg.dependencies) {
      const target = dependency.path && byDir.get(relative(dependency.path));
      if (!target) continue;
      if (!dependents.has(target)) dependents.set(target, new Set());
      dependents.get(target).add(pkg.name);
    }
    if (pkg.metadata?.ci) declared.set(pkg.name, pkg.metadata.ci);
  }
  return {
    dirs,
    dependents,
    declared,
    inputs: metadata.metadata?.ci?.inputs ?? [],
    /** The package whose directory holds `file`, the innermost one if they nest. */
    packageOf(file) {
      let best;
      for (const [name, dir] of dirs) {
        if (file.startsWith(`${dir}/`) && (!best || dir.length > dirs.get(best).length)) best = name;
      }
      return best;
    },
  };
}

/** `start` and everything reachable from it through `edges`. */
function closure(start, edges) {
  const seen = new Set(start);
  const stack = [...start];
  while (stack.length) {
    for (const next of edges.get(stack.pop()) ?? []) {
      if (!seen.has(next)) {
        seen.add(next);
        stack.push(next);
      }
    }
  }
  return seen;
}

/** Whether `file` is one of `inputs`, or inside one that ends in `/`. */
function matches(file, inputs) {
  return inputs.some((input) => (input.endsWith('/') ? file.startsWith(input) : file === input));
}
