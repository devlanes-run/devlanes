use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Levels of `include` and `extends.file` followed below the `-f` files. A deeper
/// reference makes the scan give up instead of tracking part of the tree.
const MAX_DEPTH: usize = 4;

/// Files that a project's compose files make Compose read, found by parsing them.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct References {
    /// Referenced files in the order the scan met them, without duplicates.
    pub(super) files: Vec<PathBuf>,
    /// Why a read Compose makes cannot be tracked by file stamps, when one cannot.
    pub(super) untracked: Option<&'static str>,
}

impl References {
    /// Scans `files` (absolute, in `-f` order). Their relative paths resolve
    /// against `project_dir`, as Compose resolves every path of merged `-f` files
    /// against the project directory.
    pub(super) fn scan(files: &[PathBuf], project_dir: &Path) -> Self {
        let mut scan = Scan::default();
        for file in files {
            scan.file(file, project_dir, 0);
        }
        scan.references
    }
}

/// State of one [`References::scan`].
#[derive(Debug, Default)]
struct Scan {
    references: References,
    /// Files already parsed, with the directory their paths resolved against, so
    /// include and extends cycles end.
    parsed: HashSet<(PathBuf, PathBuf)>,
}

impl Scan {
    /// Parses the compose file `file` and records what it names. `base` is the
    /// directory its relative paths resolve against.
    fn file(&mut self, file: &Path, base: &Path, depth: usize) {
        if self.references.untracked.is_some()
            || !self.parsed.insert((file.to_owned(), base.to_owned()))
        {
            return;
        }
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            // Compose fails on a missing compose file, so nothing gets cached; the
            // stamp of the missing file already changes once it appears.
            Err(error) if error.kind() == ErrorKind::NotFound => return,
            Err(_) => return self.give_up("unreadable compose file"),
        };
        // JSON indented with tabs is not valid YAML.
        let model = serde_saphyr::from_str::<Value>(&text)
            .or_else(|_| serde_json::from_str::<Value>(&text));
        match model {
            Ok(model) => self.model(&model, base, depth),
            Err(_) => self.give_up("unparsable compose file"),
        }
    }

    /// Records the files one parsed compose file names.
    fn model(&mut self, model: &Value, base: &Path, depth: usize) {
        match model.get("include") {
            Some(Value::Array(entries)) => {
                for entry in entries {
                    self.include(entry, base, depth);
                }
            }
            Some(entry @ Value::String(_)) => self.include(entry, base, depth),
            _ => {}
        }
        let Some(Value::Object(services)) = model.get("services") else {
            return;
        };
        for service in services.values() {
            let mut named = paths(service.get("env_file"));
            named.extend(paths(service.get("label_file")));
            for raw in named {
                if let Some(path) = self.resolve(base, raw) {
                    self.track(path);
                }
            }
            let extends = service
                .get("extends")
                .and_then(|extends| extends.get("file"));
            if let Some(path) = extends
                .and_then(Value::as_str)
                .and_then(|raw| self.resolve(base, raw))
            {
                // The extended file's own relative paths resolve against its directory.
                let dir = path.parent().unwrap_or(base).to_owned();
                self.nested(path, &dir, depth);
            }
        }
    }

    /// Records one `include` entry, short (`- other.yaml`) or long (`- path: ...`).
    /// Its paths, `project_directory` and `env_file` resolve against the including
    /// file's `base`. The included files resolve their own paths against the
    /// project directory, which defaults to the first path's directory, and read
    /// `.env` there when the entry names no `env_file`.
    fn include(&mut self, entry: &Value, base: &Path, depth: usize) {
        let (raw_paths, project_dir, env_files) = match entry {
            Value::String(path) => (vec![path.as_str()], None, Vec::new()),
            Value::Object(entry) => (
                paths(entry.get("path")),
                entry.get("project_directory").and_then(Value::as_str),
                paths(entry.get("env_file")),
            ),
            _ => return,
        };
        let included: Vec<PathBuf> = raw_paths
            .into_iter()
            .filter_map(|raw| self.resolve(base, raw))
            .collect();
        let Some(first) = included.first() else {
            return;
        };
        let project_dir = match project_dir {
            Some(raw) => match self.resolve(base, raw) {
                Some(dir) => dir,
                None => return,
            },
            None => first.parent().unwrap_or(base).to_owned(),
        };
        if env_files.is_empty() {
            self.track(project_dir.join(".env"));
        }
        for raw in env_files {
            if let Some(path) = self.resolve(base, raw) {
                self.track(path);
            }
        }
        for path in included {
            self.nested(path, &project_dir, depth);
        }
    }

    /// Tracks the compose file `path`, named by a file at `depth`, and scans it
    /// with `base` for its relative paths.
    fn nested(&mut self, path: PathBuf, base: &Path, depth: usize) {
        self.track(path.clone());
        if depth >= MAX_DEPTH {
            return self.give_up("compose files nested too deep");
        }
        self.file(&path, base, depth + 1);
    }

    /// `raw` as an absolute path, or `None` when the scan cannot know which file
    /// Compose reads for it (which also ends the scan) or the path names no file.
    fn resolve(&mut self, base: &Path, raw: &str) -> Option<PathBuf> {
        // Compose skips it for `include.env_file`; its stamp would mean nothing.
        if raw == "/dev/null" {
            return None;
        }
        // Interpolation can pick another file per variable value.
        if raw.contains('$') {
            self.give_up("interpolated file path");
            return None;
        }
        // Compose expands `~` to the home directory, which this scan does not look up.
        if raw.starts_with('~') {
            self.give_up("home-relative file path");
            return None;
        }
        // Compose fetches `oci://` and Git includes and extends; their content has no
        // local stamp.
        if raw.contains("://") || raw.starts_with("git@") {
            self.give_up("remote file reference");
            return None;
        }
        Some(base.join(raw))
    }

    /// Adds `path` once.
    fn track(&mut self, path: PathBuf) {
        if !self.references.files.contains(&path) {
            self.references.files.push(path);
        }
    }

    /// Marks the project untracked for `reason`, keeping the first reason.
    fn give_up(&mut self, reason: &'static str) {
        self.references.untracked.get_or_insert(reason);
    }
}

/// Path strings of a field that holds a string, a list of strings, or a list of
/// `{path: ...}` objects whose `path` is a string or a list of strings.
fn paths(value: Option<&Value>) -> Vec<&str> {
    let items = match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(item) => vec![item],
        None => Vec::new(),
    };
    items
        .into_iter()
        .flat_map(|item| match item {
            Value::String(path) => vec![path.as_str()],
            Value::Object(entry) => match entry.get("path") {
                Some(Value::String(path)) => vec![path.as_str()],
                Some(Value::Array(paths)) => paths.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files` (relative path, content) under a new temporary directory.
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (path, content) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, content).expect("write");
        }
        dir
    }

    /// Scans `compose.yaml` in `dir` and returns the referenced paths relative to
    /// `dir`, with `/` separators.
    fn scan(dir: &Path) -> (Vec<String>, Option<&'static str>) {
        let scanned = References::scan(&[dir.join("compose.yaml")], dir);
        let files = scanned
            .files
            .iter()
            .map(|path| {
                let relative = path.strip_prefix(dir).expect("inside the tree");
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        (files, scanned.untracked)
    }

    #[test]
    fn env_file_forms() {
        let dir = tree(&[(
            "compose.yaml",
            "services:\n  a:\n    env_file: a.env\n  b:\n    env_file:\n      - ./b.env\n      - path: c.env\n        required: false\n      - path: config/d.env\n",
        )]);
        let (files, untracked) = scan(dir.path());
        assert_eq!(files, ["a.env", "b.env", "c.env", "config/d.env"]);
        assert_eq!(untracked, None);
    }

    #[test]
    fn label_file_forms() {
        let dir = tree(&[(
            "compose.yaml",
            "services:\n  a:\n    label_file: a.labels\n  b:\n    label_file: [b.labels, c.labels]\n",
        )]);
        assert_eq!(scan(dir.path()).0, ["a.labels", "b.labels", "c.labels"]);
    }

    #[test]
    fn include_short_and_long() {
        let dir = tree(&[
            (
                "compose.yaml",
                "include:\n  - shared/db.yaml\n  - path: [cache/compose.yaml, cache/override.yaml]\n    env_file: cache.env\n  - path: tools/compose.yaml\n    project_directory: tools-root\n",
            ),
            (
                "shared/db.yaml",
                "services:\n  db:\n    image: postgres\n    env_file: db.env\n",
            ),
            (
                "cache/compose.yaml",
                "services:\n  redis:\n    image: redis\n",
            ),
            (
                "cache/override.yaml",
                "services:\n  redis:\n    env_file: redis.env\n",
            ),
            (
                "tools/compose.yaml",
                "services:\n  tool:\n    image: acme/tool\n    label_file: tool.labels\n",
            ),
        ]);
        let (files, untracked) = scan(dir.path());
        assert_eq!(
            files,
            [
                // Short syntax: the included file's directory is its project directory.
                "shared/.env",
                "shared/db.yaml",
                "shared/db.env",
                "cache.env",
                "cache/compose.yaml",
                "cache/override.yaml",
                "cache/redis.env",
                "tools-root/.env",
                "tools/compose.yaml",
                "tools-root/tool.labels",
            ]
        );
        assert_eq!(untracked, None);
    }

    #[test]
    fn extends_file_resolves_from_its_own_directory() {
        let dir = tree(&[
            (
                "compose.yaml",
                "services:\n  api:\n    extends:\n      file: base/common.yaml\n      service: app\n",
            ),
            (
                "base/common.yaml",
                "services:\n  app:\n    image: acme/app\n    env_file: app.env\n    extends:\n      file: ../root.yaml\n      service: root\n",
            ),
            ("root.yaml", "services:\n  root:\n    image: acme/root\n"),
        ]);
        let (files, untracked) = scan(dir.path());
        assert_eq!(
            files,
            ["base/common.yaml", "base/app.env", "base/../root.yaml"]
        );
        assert_eq!(untracked, None);
    }

    #[test]
    fn unresolvable_paths_make_it_untracked() {
        let dir = tree(&[(
            "compose.yaml",
            "include:\n  - ${STACK}/compose.yaml\nservices:\n  a:\n    image: redis\n",
        )]);
        assert_eq!(scan(dir.path()).1, Some("interpolated file path"));

        let dir = tree(&[("compose.yaml", "include:\n  - oci://example.invalid/acme\n")]);
        assert_eq!(scan(dir.path()).1, Some("remote file reference"));

        let dir = tree(&[(
            "compose.yaml",
            "services:\n  a:\n    env_file: ~/acme.env\n",
        )]);
        assert_eq!(scan(dir.path()).1, Some("home-relative file path"));
    }

    #[test]
    fn unparsable_file_makes_it_untracked() {
        let dir = tree(&[("compose.yaml", "services:\n  a: [unclosed\n")]);
        assert_eq!(scan(dir.path()).1, Some("unparsable compose file"));
    }

    #[test]
    fn include_cycles_end_and_depth_is_bounded() {
        let dir = tree(&[
            ("compose.yaml", "include:\n  - other.yaml\n"),
            ("other.yaml", "include:\n  - compose.yaml\n"),
        ]);
        let (files, untracked) = scan(dir.path());
        assert_eq!(files, [".env", "other.yaml", "compose.yaml"]);
        assert_eq!(untracked, None);

        let dir = tree(&[
            ("compose.yaml", "include:\n  - a/c.yaml\n"),
            ("a/c.yaml", "include:\n  - b/c.yaml\n"),
            ("a/b/c.yaml", "include:\n  - c/c.yaml\n"),
            ("a/b/c/c.yaml", "include:\n  - d/c.yaml\n"),
            ("a/b/c/d/c.yaml", "include:\n  - e/c.yaml\n"),
            ("a/b/c/d/e/c.yaml", "services: {}\n"),
        ]);
        assert_eq!(scan(dir.path()).1, Some("compose files nested too deep"));
    }

    #[test]
    fn flow_style_and_json() {
        let dir = tree(&[(
            "compose.yaml",
            "services: { api: { image: acme/api, env_file: [.env.api] }, web: { label_file: web.labels } }\n",
        )]);
        assert_eq!(scan(dir.path()).0, [".env.api", "web.labels"]);

        let dir = tree(&[(
            "compose.yaml",
            "{\n\t\"services\": {\n\t\t\"api\": {\"image\": \"acme/api\", \"env_file\": [{\"path\": \"api.env\"}]}\n\t}\n}\n",
        )]);
        assert_eq!(scan(dir.path()), (vec!["api.env".to_owned()], None));
    }

    #[test]
    fn compose_tags_and_anchors_parse() {
        let dir = tree(&[(
            "compose.yaml",
            "x-common: &common\n  env_file: common.env\nservices:\n  api:\n    <<: *common\n    image: acme/api\n    ports: !reset []\n    labels: !override\n      a: b\n",
        )]);
        assert_eq!(scan(dir.path()), (vec!["common.env".to_owned()], None));
    }
}
