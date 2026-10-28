# devlanes-git

Git worktree discovery for devlanes. The daemon uses it to list the worktrees that become lanes and watch for worktrees being added or removed. It runs read-only `git` commands and never creates or changes a worktree. Finding a project's `devlanes.yaml` lives in `devlanes-config`.

## Usage

```rust
use std::path::Path;
use devlanes_git::{Error, discover, watch};

async fn lanes(cwd: &Path) -> Result<(), Error> {
    let Some(repository) = discover(cwd, None).await? else { return Ok(()) };
    println!("{}", repository.primary_branch);
    for worktree in &repository.worktrees {
        println!("{} {:?} {}", worktree.identity, worktree.branch, worktree.path.display());
    }
    let mut changes = watch(repository)?;
    while let Some(worktrees) = changes.recv().await {
        println!("{} worktrees", worktrees?.len());
    }
    Ok(())
}
```

## API

- `discover`: the repository's common directory, root, primary branch and worktrees; `None` outside Git.
- `list` and `parse_porcelain`: the worktree list from `git worktree list --porcelain -z`.
- `watch`: a channel that receives the full worktree list again after each change.

## Config example

```yaml
# devlanes.yaml
lanes:
  # Branch that gets lane 0.
  primary: main
```

## Behaviour

A worktree's `identity` is its administration name under the common Git directory, so `git worktree move` keeps the same lane. The common directory tells apart repositories, so devlanes can refuse a second repository that uses a project name already taken. Without an explicit primary branch, `discover` takes origin's default branch, then `main`, then `master`.

`watch` relists on filesystem events and on a timer, because notifications can be dropped.

## License

Apache-2.0
