use std::path::PathBuf;

/// Environment variable holding [`LaneFacts::data_path`], set for every unit.
pub const LANE_DATA_ENV: &str = "DEVLANES_LANE_DATA";

/// What the daemon knows about one lane before resolving its config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneFacts {
    /// `project` from the config.
    pub project: String,
    /// Worktree of lane 0 (`project.primary_path`).
    pub primary_path: PathBuf,
    pub index: u32,
    /// `main` for lane 0, else `lane<N>` (see [`lane_name`]).
    pub name: String,
    /// Branch slug for display, or the lane name on a detached HEAD.
    pub label: String,
    /// The lane's worktree directory.
    pub path: PathBuf,
    /// The directory of the project file in this worktree, which relative `cwd`,
    /// `run_script.file` and `sync` paths start from. The worktree itself unless the
    /// project file sits below it (a subdirectory project).
    pub project_dir: PathBuf,
    /// The lane's data folder outside the worktree (`lane.data_path`). It belongs to
    /// this lane alone, also when another lane runs from the same worktree.
    pub data_path: PathBuf,
    /// Checked-out branch; `None` on a detached HEAD or outside git.
    pub branch: Option<String>,
    /// What the lane's default domain and `domain()` end in: `localhost`, or
    /// the TLD under the `resolver` strategy.
    pub domain_suffix: String,
}

impl LaneFacts {
    /// Derives `name` and `label` from the index and branch.
    pub fn new(
        project: impl Into<String>,
        primary_path: impl Into<PathBuf>,
        index: u32,
        path: impl Into<PathBuf>,
        data_path: impl Into<PathBuf>,
        branch: Option<String>,
    ) -> Self {
        let name = lane_name(index);
        let label = branch.as_deref().map_or_else(|| name.clone(), branch_label);
        let path = path.into();
        Self {
            project: project.into(),
            primary_path: primary_path.into(),
            index,
            name,
            label,
            project_dir: path.clone(),
            path,
            data_path: data_path.into(),
            branch,
            domain_suffix: devlanes_config::DEFAULT_DOMAIN_SUFFIX.to_owned(),
        }
    }

    /// Sets [`LaneFacts::project_dir`] to `relative`, the project file's directory
    /// below the worktree.
    pub fn with_project_dir(mut self, relative: &std::path::Path) -> Self {
        self.project_dir = self.path.join(relative);
        self
    }

    /// Sets [`LaneFacts::domain_suffix`], such as the `resolver` strategy's TLD.
    pub fn with_domain_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.domain_suffix = suffix.into();
        self
    }
}

/// Returns `main` for lane 0 and `lane<N>` otherwise.
pub fn lane_name(index: u32) -> String {
    if index == 0 {
        "main".to_owned()
    } else {
        format!("lane{index}")
    }
}

/// Turns a branch name into a display label: `/` becomes `-`.
pub fn branch_label(branch: &str) -> String {
    branch.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Names follow the index; labels follow the branch with slashes replaced.
    fn names_and_labels() {
        let main = LaneFacts::new("acme", "/w", 0, "/w", "/d/1", Some("main".into()));
        assert_eq!((main.name.as_str(), main.label.as_str()), ("main", "main"));
        let lane = LaneFacts::new("acme", "/w", 3, "/x", "/d/2", Some("feat/a/b".into()));
        assert_eq!(
            (lane.name.as_str(), lane.label.as_str()),
            ("lane3", "feat-a-b")
        );
        let detached = LaneFacts::new("acme", "/w", 2, "/y", "/d/3", None);
        assert_eq!(detached.label, "lane2");
    }
}
