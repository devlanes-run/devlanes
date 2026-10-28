//! Finding the project file for a directory: the nearest `devlanes.yaml` up to the
//! repository root. Other project files are only ever named with `--file`, so nothing
//! here lists or parses them.

use std::path::{Path, PathBuf};

/// File name devlanes looks for when no `--file` is given.
pub const DEFAULT_PROJECT_FILE: &str = "devlanes.yaml";

/// Where a search for `devlanes.yaml` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSearch {
    Found(PathBuf),
    /// No config; `searched_to` is the last directory looked in (the repository
    /// root inside Git, else the filesystem root).
    Missing {
        searched_to: PathBuf,
    },
}

/// Searches `cwd` and its ancestors so monorepo configs can live below the Git root,
/// stopping at the repository root (the first directory holding `.git`): a config above
/// it belongs to some other project. Blocking: checks the filesystem.
pub fn search_project_file(cwd: &Path) -> std::io::Result<ConfigSearch> {
    let mut searched_to = cwd.to_path_buf();
    for parent in cwd.ancestors() {
        searched_to = parent.to_path_buf();
        let path = parent.join(DEFAULT_PROJECT_FILE);
        if path.try_exists()? {
            return Ok(ConfigSearch::Found(path));
        }
        if parent.join(".git").try_exists()? {
            break;
        }
    }
    Ok(ConfigSearch::Missing { searched_to })
}

/// [`search_project_file`] without the place the search ended.
pub fn find_project_file(cwd: &Path) -> std::io::Result<Option<PathBuf>> {
    Ok(match search_project_file(cwd)? {
        ConfigSearch::Found(path) => Some(path),
        ConfigSearch::Missing { .. } => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The search walks up to the nearest config and never past the repository root.
    fn search_stops_at_the_repository_root() {
        let dir = tempfile::tempdir().expect("dir");
        let outer = dir.path();
        std::fs::write(outer.join(DEFAULT_PROJECT_FILE), "project: outer\n").expect("outer");
        let repo = outer.join("repo");
        let nested = repo.join("apps/web/src");
        std::fs::create_dir_all(repo.join(".git")).expect("git");
        std::fs::create_dir_all(&nested).expect("nested");
        assert_eq!(
            search_project_file(&nested).expect("search"),
            ConfigSearch::Missing {
                searched_to: repo.clone()
            }
        );
        std::fs::write(repo.join("apps/devlanes.yaml"), "project: apps\n").expect("apps");
        assert_eq!(
            find_project_file(&nested).expect("find"),
            Some(repo.join("apps/devlanes.yaml"))
        );
    }
}
