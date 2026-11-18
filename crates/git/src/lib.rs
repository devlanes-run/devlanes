//! Git worktree discovery preserves administration identities across directory moves.

use devlanes_core::paths::{forward_slashes, path_starts_with, same_path, strip_path_prefix};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

/// Git failures retain stderr for diagnostics without exposing a request environment.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Git(String),
    #[error("git output is not valid UTF-8")]
    Encoding,
    #[error("malformed worktree output: {0}")]
    Porcelain(String),
    #[error(
        "worktree {} has lost its .git entry; restore it or run `git worktree prune`",
        .0.display()
    )]
    MissingGitEntry(PathBuf),
    #[error("watch setup failed: {0}")]
    Watch(#[from] notify::Error),
}

/// The administration name is stable when `git worktree move` changes its path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Worktree {
    pub identity: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    pub present: bool,
    pub prunable: bool,
}

/// The common Git directory tells repositories apart, so a second one cannot take a
/// project name another repository uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub common_dir: PathBuf,
    pub root: PathBuf,
    pub primary_branch: String,
    pub worktrees: Vec<Worktree>,
}

/// Returns `None` for an ordinary non-Git directory; other failures remain errors.
pub async fn discover(cwd: &Path, primary: Option<&str>) -> Result<Option<Repository>, Error> {
    let probe = Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .await?;
    if !probe.status.success() {
        let stderr = String::from_utf8_lossy(&probe.stderr);
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(Error::Git(stderr.trim().into()));
    }
    let root = native(text(&probe.stdout)?.trim_end_matches('\n'));
    let common_dir = native(
        text(
            &git(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        )?
        .trim_end_matches('\n'),
    );
    let primary_branch = match primary {
        Some(branch) => branch.to_owned(),
        None => default_branch(&root).await?,
    };
    let worktrees = list(&root, &common_dir).await?;
    // Inside a linked worktree that lost its `.git` entry, Git answers for the
    // enclosing main checkout; refuse instead of treating it as part of `root`.
    let canonical = tokio::fs::canonicalize(cwd)
        .await
        .unwrap_or_else(|_| cwd.to_owned());
    // The comparisons ignore case and `\\?\` on Windows, where canonical paths
    // are verbatim and Git's are not.
    if let Some(broken) = worktrees.iter().find(|tree| {
        !tree.present && !same_path(&tree.path, &root) && path_starts_with(&canonical, &tree.path)
    }) {
        return Err(Error::MissingGitEntry(broken.path.clone()));
    }
    Ok(Some(Repository {
        common_dir,
        root,
        primary_branch,
        worktrees,
    }))
}

/// Re-listing is authoritative; filesystem notifications only trigger this operation.
pub async fn list(root: &Path, common_dir: &Path) -> Result<Vec<Worktree>, Error> {
    let output = git(root, &["worktree", "list", "--porcelain", "-z"]).await?;
    let mut worktrees = parse_porcelain(&output)?;
    for worktree in &mut worktrees {
        worktree.present = tokio::fs::try_exists(&worktree.path).await? && !worktree.prunable;
        if worktree.present {
            // `.git` can vanish after the listing (a race with `git worktree remove`).
            let Some(admin) = admin_dir(&worktree.path).await? else {
                worktree.present = false;
                continue;
            };
            worktree.identity = if same_path(&admin, common_dir) {
                "main".into()
            } else {
                // `/`-separated on Windows too, matching the `worktrees/<name>`
                // spelling below: identities are state keys.
                forward_slashes(strip_path_prefix(&admin, common_dir).ok_or_else(|| {
                    Error::Porcelain("worktree admin directory outside common directory".into())
                })?)
            };
        }
    }
    // Missing entries still have administration records until Git prunes them.
    let admins = common_dir.join("worktrees");
    if let Ok(mut entries) = tokio::fs::read_dir(&admins).await {
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path().join("gitdir");
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                let gitfile = native(content.trim_end_matches('\n'));
                if let Some(worktree) = worktrees
                    .iter_mut()
                    .find(|tree| same_path(&tree.path, gitfile.parent().unwrap_or(Path::new(""))))
                {
                    worktree.identity =
                        format!("worktrees/{}", entry.file_name().to_string_lossy());
                }
            }
        }
    }
    Ok(worktrees)
}

/// The absolute administration directory of the worktree at `path`, or `None`
/// when `path` no longer holds its `.git` entry. Read from that entry rather
/// than with `git -C <path>`: a process working in the worktree keeps Windows
/// from renaming it, so a re-list during `git worktree move` would make the
/// move fail. Reading the entry also never climbs to an enclosing checkout.
async fn admin_dir(path: &Path) -> Result<Option<PathBuf>, Error> {
    let entry = path.join(".git");
    let metadata = match tokio::fs::metadata(&entry).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // The main checkout keeps its repository in `.git`; linked worktrees (and
    // checkouts made with `--separate-git-dir`) have a `gitdir:` file.
    if metadata.is_dir() {
        return Ok(Some(entry));
    }
    let content = match tokio::fs::read(&entry).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    gitdir_target(path, text(&content)?).map(Some)
}

/// The directory a `.git` file of the worktree at `path` names: `gitdir:
/// <path>`, absolute or (with `worktree.useRelativePaths`) relative to the
/// worktree, with `.` and `..` resolved without touching the filesystem.
fn gitdir_target(path: &Path, content: &str) -> Result<PathBuf, Error> {
    let target = content
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| {
            Error::Git(format!(
                "{} is not a gitdir file",
                path.join(".git").display()
            ))
        })?;
    let joined = path.join(native(target));
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    Ok(resolved)
}

/// NUL records preserve spaces and newlines in worktree paths.
pub fn parse_porcelain(bytes: &[u8]) -> Result<Vec<Worktree>, Error> {
    let mut result = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut bare = false;
    for field in bytes.split(|byte| *byte == 0) {
        let field = text(field)?;
        if field.is_empty() {
            if let Some(worktree) = current.take()
                && !bare
            {
                result.push(worktree);
            }
            bare = false;
        } else if let Some(path) = field.strip_prefix("worktree ") {
            current = Some(Worktree {
                identity: String::new(),
                path: native(path),
                branch: None,
                head: String::new(),
                present: false,
                prunable: false,
            });
        } else if field == "bare" {
            bare = true;
        } else if let Some(worktree) = current.as_mut() {
            if let Some(head) = field.strip_prefix("HEAD ") {
                worktree.head = head.into();
            }
            if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                worktree.branch = Some(branch.into());
            }
            if field == "prunable" || field.starts_with("prunable ") {
                worktree.prunable = true;
            }
        } else {
            return Err(Error::Porcelain("field before worktree path".into()));
        }
    }
    if let Some(worktree) = current
        && !bare
    {
        result.push(worktree);
    }
    Ok(result)
}

/// Sends a fresh [`list`] after a change that can alter it, and every 30 seconds,
/// which covers dropped notifications and changes only `head` shows (commits).
/// Blocking: registers the watches and resolves their canonical paths.
pub fn watch(
    repository: Repository,
) -> Result<tokio::sync::mpsc::Receiver<Result<Vec<Worktree>, Error>>, Error> {
    use notify::Watcher;
    let scope = Scope::of(&repository);
    let (events, mut changes) = tokio::sync::mpsc::channel(1);
    let mut watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            if event.is_ok_and(|event| changes_worktrees(&event, &scope)) {
                let _ = events.try_send(());
            }
        })?;
    watcher.watch(&repository.common_dir, notify::RecursiveMode::Recursive)?;
    for worktree in &repository.worktrees {
        if worktree.present {
            watcher.watch(&worktree.path, notify::RecursiveMode::NonRecursive)?;
        }
    }
    let (updates, receiver) = tokio::sync::mpsc::channel(2);
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = updates.closed() => break,
                _ = tick.tick() => {},
                Some(()) = changes.recv() => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    while changes.try_recv().is_ok() {}
                }
            }
            if updates
                .send(list(&repository.root, &repository.common_dir).await)
                .await
                .is_err()
            {
                break;
            }
        }
    });
    Ok(receiver)
}

/// The watched directories, each as given and canonical: FSEvents (macOS)
/// reports canonical paths, the other backends the watched path joined with
/// the name that changed.
struct Scope {
    common: Vec<PathBuf>,
    roots: Vec<PathBuf>,
}

impl Scope {
    /// The common dir and the present worktrees of `repository`.
    fn of(repository: &Repository) -> Self {
        let both = |path: &Path| {
            let mut forms = vec![path.to_owned()];
            if let Ok(canonical) = std::fs::canonicalize(path)
                && canonical != path
            {
                forms.push(canonical);
            }
            forms
        };
        Self {
            common: both(&repository.common_dir),
            roots: repository
                .worktrees
                .iter()
                .filter(|tree| tree.present)
                .flat_map(|tree| both(&tree.path))
                .collect(),
        }
    }
}

/// Whether a watch event can change the worktree list. Git writes the common
/// dir all the time (index, objects, logs, lock files, `FETCH_HEAD`, for every
/// `git status` an agent runs), and the listing itself opens files there, which
/// inotify reports; relisting on those would keep the daemon busy. What the
/// list shows comes from `HEAD`, the admin dirs under `worktrees/` with their
/// `HEAD`, `gitdir` and `locked`, and each worktree folder and its `.git`
/// entry. A rescan, or an event without a path or with one outside the
/// watches, counts as a change.
fn changes_worktrees(event: &notify::Event, scope: &Scope) -> bool {
    use notify::{EventKind, event::ModifyKind};
    if event.kind.is_access() {
        return false;
    }
    if event.need_rescan() || event.paths.is_empty() {
        return true;
    }
    // A directory's own write time changes with every entry made in it, which
    // Windows reports as a modification of the directory.
    let reshapes = matches!(
        event.kind,
        EventKind::Any
            | EventKind::Other
            | EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_))
    );
    let names = |rest: &Path| -> Vec<String> {
        rest.iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect()
    };
    event.paths.iter().any(|path| {
        if let Some(rest) = scope
            .common
            .iter()
            .find_map(|common| strip_path_prefix(path, common))
        {
            return match names(rest).as_slice() {
                [] => reshapes,
                [name] => name == "HEAD" || (name == "worktrees" && reshapes),
                [worktrees, _] => worktrees == "worktrees" && reshapes,
                [worktrees, _, file] => {
                    worktrees == "worktrees"
                        && matches!(file.as_str(), "HEAD" | "gitdir" | "locked")
                }
                _ => false,
            };
        }
        // Linked worktrees usually sit inside the main one, so a path can be
        // below several roots; it matters when it matters to any of them.
        let mut inside = false;
        for root in &scope.roots {
            if let Some(rest) = strip_path_prefix(path, root) {
                inside = true;
                match names(rest).as_slice() {
                    [] if reshapes => return true,
                    [name] if name == ".git" => return true,
                    _ => {}
                }
            }
        }
        !inside
    })
}

/// Uses origin's symbolic default before falling back to conventional local branches.
async fn default_branch(root: &Path) -> Result<String, Error> {
    if let Ok(output) = git(
        root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )
    .await
        && let Some(branch) = text(&output)?.trim().strip_prefix("refs/remotes/origin/")
    {
        return Ok(branch.into());
    }
    for branch in ["main", "master"] {
        if git(
            root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .await
        .is_ok()
        {
            return Ok(branch.into());
        }
    }
    Ok("main".into())
}

/// Helper subprocesses use Tokio's own reaping independently of managed units.
async fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(Error::Git(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ))
    }
}

/// Rejects undecodable Git metadata rather than silently changing an identity.
fn text(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| Error::Encoding)
}

/// A path printed by Git, as is on Unix.
#[cfg(unix)]
fn native(path: &str) -> PathBuf {
    PathBuf::from(path)
}

/// A path printed by Git, with native separators. Git for Windows prints
/// `C:/x/y`; worktree paths are stored as state keys and shown to users, so
/// they take the spelling Windows tools and canonical cwds use.
#[cfg(windows)]
fn native(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_paths_and_detached_heads_survive() -> Result<(), Error> {
        let trees = parse_porcelain(b"worktree /repo with\nnewline\0HEAD abc\0branch refs/heads/feat/checkout\0\0worktree /detached\0HEAD def\0detached\0\0")?;
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[0].path, Path::new("/repo with\nnewline"));
        assert_eq!(trees[0].branch.as_deref(), Some("feat/checkout"));
        assert!(trees[1].branch.is_none());
        Ok(())
    }

    #[test]
    fn bare_entries_do_not_become_lanes() -> Result<(), Error> {
        assert!(parse_porcelain(b"worktree /bare.git\0bare\0\0")?.is_empty());
        Ok(())
    }

    /// Keeps the test body readable while using a canonicalized directory.
    struct TempPath(PathBuf);

    impl TempPath {
        /// Mirrors `TempDir::path` for the canonical location.
        fn path(&self) -> &Path {
            &self.0
        }
    }

    /// Adds a linked worktree on a new branch. Only tests create worktrees;
    /// devlanes itself never writes to a repository.
    async fn add(root: &Path, destination: &Path, branch: &str) -> Result<(), Error> {
        let destination = destination.to_str().ok_or(Error::Encoding)?;
        git(root, &["worktree", "add", "-b", branch, "--", destination]).await?;
        Ok(())
    }

    /// Creates a repository with one commit and a linked worktree under
    /// `.worktrees/lane`, the layout devlanes uses.
    async fn repository_with_nested_lane(root: &Path) -> Result<PathBuf, Error> {
        git(root, &["init", "-b", "main"]).await?;
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        )
        .await?;
        let lane = root.join(".worktrees/lane");
        add(root, &lane, "feat/nested").await?;
        Ok(lane)
    }

    /// When `.git` vanishes after `git worktree list`, the admin lookup must
    /// not climb into the enclosing main checkout.
    #[tokio::test]
    async fn admin_lookup_never_answers_for_the_enclosing_checkout()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            let temp = TempPath(dunce::canonicalize(dir.path())?);
            let lane = repository_with_nested_lane(temp.path()).await?;
            let common = temp.path().join(".git");
            let admin = admin_dir(&lane).await?.ok_or("the worktree has its link")?;
            assert!(admin.starts_with(common.join("worktrees")), "{admin:?}");
            std::fs::remove_file(lane.join(".git"))?;
            // Git run there would answer for the main checkout.
            let outer = git(&lane, &["rev-parse", "--path-format=absolute", "--git-dir"]).await?;
            assert_eq!(native(text(&outer)?.trim_end_matches('\n')), common);
            assert_eq!(admin_dir(&lane).await?, None);
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }

    /// The main checkout's `.git` directory and a linked worktree's `gitdir:`
    /// file, absolute or relative, name the same directories Git reports.
    #[tokio::test]
    async fn admin_dir_reads_the_git_entry() -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            let temp = TempPath(dunce::canonicalize(dir.path())?);
            let lane = repository_with_nested_lane(temp.path()).await?;
            let main = admin_dir(temp.path()).await?.ok_or("main has .git")?;
            assert_eq!(main, temp.path().join(".git"));
            let reported =
                git(&lane, &["rev-parse", "--path-format=absolute", "--git-dir"]).await?;
            let expected = native(text(&reported)?.trim_end_matches('\n'));
            let linked = admin_dir(&lane).await?.ok_or("the lane has .git")?;
            assert!(same_path(&linked, &expected), "{linked:?} != {expected:?}");
            std::fs::write(lane.join(".git"), "gitdir: ../../.git/worktrees/lane\r\n")?;
            let relative = admin_dir(&lane).await?.ok_or("the lane has .git")?;
            assert!(
                same_path(&relative, &expected),
                "{relative:?} != {expected:?}"
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }

    #[test]
    fn gitdir_files_resolve_or_fail() {
        let worktree = Path::new("/src/acme/.worktrees/lane");
        assert_eq!(
            gitdir_target(worktree, "gitdir: ./../../.git/worktrees/lane\n").ok(),
            Some(Path::new("/src/acme/.git/worktrees/lane").to_path_buf())
        );
        assert!(gitdir_target(worktree, "").is_err());
        assert!(gitdir_target(worktree, "gitdir:   \n").is_err());
        assert!(gitdir_target(worktree, "ref: refs/heads/main\n").is_err());
    }

    /// Discovery from inside a worktree without its `.git` entry is an error,
    /// never the main checkout.
    #[tokio::test]
    async fn discover_refuses_a_worktree_without_its_git_entry()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            let temp = TempPath(dunce::canonicalize(dir.path())?);
            let lane = repository_with_nested_lane(temp.path()).await?;
            let nested = lane.join("apps");
            std::fs::create_dir(&nested)?;
            assert!(discover(&nested, None).await?.is_some());
            std::fs::remove_file(lane.join(".git"))?;
            let error = discover(&nested, None).await.err().ok_or("discovered")?;
            assert!(
                matches!(&error, Error::MissingGitEntry(path) if path == &lane),
                "{error}"
            );
            let main = discover(temp.path(), None)
                .await?
                .ok_or("missing repository")?;
            let tree = main
                .worktrees
                .iter()
                .find(|tree| tree.path == lane)
                .ok_or("missing lane")?;
            assert!(!tree.present);
            assert_eq!(tree.identity, "worktrees/lane");
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    /// Git for Windows prints `/`; stored paths use `\`.
    fn windows_porcelain_paths_use_backslashes() -> Result<(), Error> {
        let trees = parse_porcelain(b"worktree C:/src/acme lanes/web\0HEAD abc\0\0")?;
        assert_eq!(trees[0].path.as_os_str(), r"C:\src\acme lanes\web");
        Ok(())
    }

    #[tokio::test]
    async fn moved_worktree_keeps_its_identity() -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            // Git reports canonical paths, while macOS temp paths include a /var
            // symlink; dunce avoids the `\\?\` form Git rejects on Windows.
            let temp = TempPath(dunce::canonicalize(dir.path())?);
            git(temp.path(), &["init", "-b", "main"]).await?;
            git(
                temp.path(),
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "initial",
                ],
            )
            .await?;
            let lane = temp.path().join("lane");
            add(temp.path(), &lane, "feat/checkout").await?;
            let before = discover(temp.path(), None)
                .await?
                .ok_or("missing repository")?;
            let linked = before
                .worktrees
                .iter()
                .find(|tree| tree.branch.as_deref() == Some("feat/checkout"))
                .ok_or("missing lane")?;
            let moved = temp.path().join("moved");
            let output = Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(["worktree", "move"])
                .arg(&lane)
                .arg(&moved)
                .output()
                .await?;
            assert!(output.status.success());
            let after = discover(temp.path(), None)
                .await?
                .ok_or("missing repository")?;
            let moved = after
                .worktrees
                .iter()
                .find(|tree| tree.path == moved)
                .ok_or("missing moved lane")?;
            assert_eq!(linked.identity, moved.identity);
            assert!(
                moved.identity.starts_with("worktrees/"),
                "{}",
                moved.identity
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }

    #[test]
    /// Only events that can change what `list` reports wake the watcher: not
    /// reads, not the index, objects or logs Git writes on every command, not
    /// other files in a worktree folder.
    fn only_list_changes_wake_the_watcher() {
        use notify::{
            Event, EventKind,
            event::{
                AccessKind, AccessMode, CreateKind, DataChange, Flag, ModifyKind, RemoveKind,
                RenameMode,
            },
        };
        let root = Path::new("/src/acme");
        let lane = root.join(".worktrees/lane");
        let scope = Scope {
            common: vec![root.join(".git")],
            roots: vec![root.to_owned(), lane.clone()],
        };
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        // Windows reports writes as `Modify(Any)`, and a directory whose
        // entries changed as one too.
        let touched = EventKind::Modify(ModifyKind::Any);
        let renamed = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
        let file = EventKind::Create(CreateKind::File);
        let gone = EventKind::Remove(RemoveKind::Folder);
        let read = EventKind::Access(AccessKind::Close(AccessMode::Read));
        let git = |rest: &str| root.join(".git").join(rest);
        let cases: Vec<(EventKind, Vec<PathBuf>, bool)> = vec![
            (write, vec![git("index")], false),
            (file, vec![git("index.lock")], false),
            (file, vec![git("objects/ab/cdef")], false),
            (write, vec![git("logs/HEAD")], false),
            (write, vec![git("FETCH_HEAD")], false),
            (write, vec![git("worktrees/lane/index")], false),
            (touched, vec![git("worktrees/lane")], false),
            (touched, vec![root.join(".git")], false),
            (read, vec![git("HEAD")], false),
            (file, vec![lane.join("notes.txt")], false),
            (touched, vec![root.to_owned()], false),
            (write, vec![git("HEAD")], true),
            (
                renamed,
                vec![git("worktrees/lane/HEAD.lock"), git("worktrees/lane/HEAD")],
                true,
            ),
            (touched, vec![git("worktrees/lane/gitdir")], true),
            (file, vec![git("worktrees/lane/locked")], true),
            (
                EventKind::Create(CreateKind::Folder),
                vec![git("worktrees/new")],
                true,
            ),
            (gone, vec![git("worktrees/lane")], true),
            (
                EventKind::Remove(RemoveKind::File),
                vec![lane.join(".git")],
                true,
            ),
            (gone, vec![lane.clone()], true),
            (write, vec![PathBuf::from("/elsewhere/file")], true),
            (EventKind::Any, Vec::new(), true),
        ];
        for (kind, paths, wakes) in cases {
            let mut event = Event::new(kind);
            event.paths = paths;
            assert_eq!(changes_worktrees(&event, &scope), wakes, "{event:?}");
        }
        let rescan = Event::new(EventKind::Other)
            .set_flag(Flag::Rescan)
            .add_path(git("index"));
        assert!(changes_worktrees(&rescan, &scope));
    }

    /// The watcher's own listing opens files in the common dir, which inotify
    /// reports, and `git status` rewrites the index: neither relists, a new
    /// worktree does.
    #[tokio::test]
    async fn only_a_list_change_wakes_the_watcher() -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            let temp = TempPath(dunce::canonicalize(dir.path())?);
            let nested = repository_with_nested_lane(temp.path()).await?;
            let repository = discover(temp.path(), None)
                .await?
                .ok_or("missing repository")?;
            let mut updates = watch(repository)?;
            updates.recv().await.ok_or("first listing")??;
            std::fs::write(nested.join("notes.txt"), "x")?;
            git(&nested, &["status"]).await?;
            git(temp.path(), &["status"]).await?;
            // A listing woken by the previous one, or by those commands, follows
            // within 100 ms of the event.
            let again = tokio::time::timeout(Duration::from_secs(1), updates.recv()).await;
            assert!(again.is_err(), "relisted without a change: {again:?}");
            let lane = temp.path().join("second");
            add(temp.path(), &lane, "feat/second").await?;
            loop {
                let trees = updates.recv().await.ok_or("listing after add")??;
                if trees.iter().any(|tree| tree.path == lane) {
                    break;
                }
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }
}
