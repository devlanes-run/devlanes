//! npm scripts on Windows, run the way npm runs them but without the npm
//! process.
//!
//! npm forwards a stop to its script's shell with `child.kill`, which on
//! Windows terminates it with code 1, and then exits 1 itself, so every stop
//! of `npm run` read as a failure. Starting the script's steps directly lets
//! the stop reach the script's program and its own exit code through.
//!
//! Each present step (`pre<name>`, the script with its arguments,
//! `post<name>`) runs as npm runs it: `cmd /d /s /c "<step>"` in the package
//! directory, stopping at the first step that fails, with that step's code.
//! The steps go into `DEVLANES_NPM_PRE`, `DEVLANES_NPM_SCRIPT` and
//! `DEVLANES_NPM_POST`, and the unit runs
//! `cmd.exe /v:on /d /s /c "cmd /d /s /c !DEVLANES_NPM_PRE! && ..."`. Delayed
//! expansion inserts each value after that line is parsed and never expands
//! the inserted text, so each step's own `cmd.exe` receives exactly npm's
//! command line, and `cd` or `set` in one step does not reach the next. A
//! `script-shell` from `.npmrc` other than `cmd.exe` gets
//! `[shell, -c, "(<pre>) && (<script>) && (<post>)"]`, whose subshells keep
//! each step's state to itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use devlanes_shell_windows::quote_for_cmd;
use serde_json::Value;

use crate::{Manifest, NpmError};

/// The variable holding the `pre<name>` step for `cmd.exe`.
pub(crate) const PRE: &str = "DEVLANES_NPM_PRE";
/// The variable holding the script step, with its arguments.
pub(crate) const SCRIPT: &str = "DEVLANES_NPM_SCRIPT";
/// The variable holding the `post<name>` step.
pub(crate) const POST: &str = "DEVLANES_NPM_POST";

/// How an npm script runs on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpmPlan {
    pub argv: Vec<String>,
    /// npm's package and lifecycle variables, plus the step variables for
    /// `cmd.exe`.
    pub env: BTreeMap<String, String>,
    /// `node_modules\.bin` of the package directory and every parent, nearest
    /// first, as npm puts them on `PATH`.
    pub path: Vec<PathBuf>,
    /// The shell the steps run in, for the launch note.
    pub shell: String,
}

/// The plan for script `name` of the package `manifest` read from
/// `manifest_path`, with `args` after the script as npm passes them.
/// `script-shell` is read from `.npmrc` in the package directory, then in
/// `worktree`, then in `home` (the user's profile directory).
pub(crate) fn npm_plan(
    manifest_path: &Path,
    manifest: &Manifest,
    worktree: &Path,
    home: Option<&Path>,
    name: &str,
    body: &str,
    args: &[String],
) -> Result<NpmPlan, NpmError> {
    let dir = manifest_path.parent().unwrap_or(worktree);
    let step = |prefix: &str| manifest.scripts.get(&format!("{prefix}{name}")).cloned();
    let (pre, post) = (step("pre"), step("post"));
    let shell = script_shell(dir, worktree, home);
    let cmd = shell.as_deref().is_none_or(is_cmd);
    let mut env = package_env(manifest_path, manifest, dir, name, body);
    let argv = if cmd {
        if let Some(unsafe_arg) = args.iter().find(|arg| arg.contains(['\r', '\n'])) {
            return Err(NpmError::UnsafeArgument(unsafe_arg.clone()));
        }
        let quoted: Vec<String> = args.iter().map(|arg| quote_for_cmd(arg)).collect();
        let script = with_args(body, &quoted);
        let (line, steps) = cmd_steps(pre.as_deref(), &script, post.as_deref());
        env.extend(steps);
        let program = shell.clone().unwrap_or_else(|| "cmd.exe".to_owned());
        vec![
            program,
            "/v:on".into(),
            "/d".into(),
            "/s".into(),
            "/c".into(),
            line,
        ]
    } else {
        let quoted: Vec<String> = args.iter().map(|arg| quote_sh(arg)).collect();
        let script = with_args(body, &quoted);
        let line = sh_line(pre.as_deref(), &script, post.as_deref());
        vec![shell.clone().unwrap_or_default(), "-c".into(), line]
    };
    Ok(NpmPlan {
        argv,
        env,
        path: dir
            .ancestors()
            .map(|ancestor| ancestor.join("node_modules").join(".bin"))
            .collect(),
        shell: shell.unwrap_or_else(|| "cmd.exe".to_owned()),
    })
}

/// `body` followed by the already quoted `args`, as npm joins them.
fn with_args(body: &str, quoted: &[String]) -> String {
    std::iter::once(body.to_owned())
        .chain(quoted.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `cmd.exe /v:on` line that runs each present step in its own
/// `cmd /d /s /c`, joined by `&&`, and the step variables it reads, each
/// holding the step quoted as npm quotes it.
pub(crate) fn cmd_steps(
    pre: Option<&str>,
    script: &str,
    post: Option<&str>,
) -> (String, BTreeMap<String, String>) {
    let mut parts = Vec::new();
    let mut steps = BTreeMap::new();
    for (variable, step) in [(PRE, pre), (SCRIPT, Some(script)), (POST, post)] {
        let Some(step) = step else { continue };
        parts.push(format!("cmd /d /s /c !{variable}!"));
        steps.insert(variable.to_owned(), format!("\"{step}\""));
    }
    (parts.join(" && "), steps)
}

/// The `-c` line for a `script-shell` other than `cmd.exe`: each present
/// step in its own subshell, joined by `&&`.
fn sh_line(pre: Option<&str>, script: &str, post: Option<&str>) -> String {
    [pre, Some(script), post]
        .into_iter()
        .flatten()
        .map(|step| format!("({step})"))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// `arg` in single quotes for a POSIX shell, as npm quotes script arguments
/// for one.
fn quote_sh(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Whether `shell` is `cmd.exe`, which npm decides by its file name.
fn is_cmd(shell: &str) -> bool {
    let name = shell.rsplit(['\\', '/']).next().unwrap_or(shell);
    name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("cmd.exe")
}

/// `script-shell` from the first `.npmrc` of `dir`, `worktree` and `home`
/// that sets it.
fn script_shell(dir: &Path, worktree: &Path, home: Option<&Path>) -> Option<String> {
    [Some(dir), Some(worktree), home]
        .into_iter()
        .flatten()
        .find_map(|dir| npmrc_value(&dir.join(".npmrc"), "script-shell"))
}

/// The value of `key` in the ini file at `path`, quotes removed; `None` when
/// the file or the key is missing.
fn npmrc_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with([';', '#']) {
            return None;
        }
        let (name, value) = line.split_once('=')?;
        if name.trim() != key {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// The variables `@npmcli/run-script` sets for a script: the package's
/// `name`, `version`, `config`, `engines` and `bin` flattened under
/// `npm_package_`, and the lifecycle ones.
fn package_env(
    manifest_path: &Path,
    manifest: &Manifest,
    dir: &Path,
    name: &str,
    body: &str,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    // npm normalizes a `bin` string to `{ <package name>: <path> }`.
    let bin = match (&manifest.bin, &manifest.name) {
        (Some(Value::String(path)), Some(Value::String(package))) => Some(Value::Object(
            [(package.clone(), Value::String(path.clone()))]
                .into_iter()
                .collect(),
        )),
        (bin, _) => bin.clone(),
    };
    for (key, value) in [
        ("name", &manifest.name),
        ("version", &manifest.version),
        ("config", &manifest.config),
        ("engines", &manifest.engines),
        ("bin", &bin),
    ] {
        if let Some(value) = value {
            flatten(&mut env, &format!("npm_package_{key}"), value);
        }
    }
    let dir = dir.to_string_lossy().into_owned();
    env.insert(
        "npm_package_json".into(),
        manifest_path.to_string_lossy().into_owned(),
    );
    env.insert("npm_lifecycle_event".into(), name.to_owned());
    env.insert("npm_lifecycle_script".into(), body.to_owned());
    env.insert("npm_command".into(), "run-script".into());
    env.insert("INIT_CWD".into(), dir);
    env
}

/// `value` under `name` as npm's `packageEnvs` flattens it: objects by key,
/// arrays by index, `null` and `false` as empty.
fn flatten(env: &mut BTreeMap<String, String>, name: &str, value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                flatten(env, &format!("{name}_{key}"), value);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(env, &format!("{name}_{index}"), item);
            }
        }
        Value::Null | Value::Bool(false) => {
            env.insert(name.to_owned(), String::new());
        }
        Value::String(text) => {
            env.insert(name.to_owned(), text.clone());
        }
        other => {
            env.insert(name.to_owned(), other.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files` under `root`, creating directories.
    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, text).expect("write");
        }
    }

    /// The plan for `name` of `<root>/web/package.json`, with `args`.
    fn planned(root: &Path, name: &str, args: &[&str]) -> NpmPlan {
        let file = root.join(r"web\package.json");
        let manifest = crate::read_manifest(&file)
            .expect("parse")
            .expect("manifest");
        let body = manifest.scripts[name].clone();
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        npm_plan(&file, &manifest, root, None, name, &body, &args).expect("plan")
    }

    /// The step variables of `plan`.
    fn steps(plan: &NpmPlan) -> Vec<(&str, &str)> {
        plan.env
            .iter()
            .filter(|(name, _)| name.starts_with("DEVLANES_NPM_"))
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }

    /// Expected values follow npm: each present `pre<name>`, script and
    /// `post<name>` step as `cmd /d /s /c "<step>"` with the arguments after
    /// the script, a non-cmd `script-shell` with `-c`, `node_modules\.bin` of
    /// the package and every parent on `PATH`, and the package's variables.
    #[test]
    fn npm_plans() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(
            root,
            &[(
                r"web\package.json",
                r#"{"name":"acme-web","version":"1.2.3","config":{"port":"8080"},
                    "scripts":{"dev":"node server.js","prebuild":"echo pre","build":"tsc",
                    "postbuild":"echo post","prelint":"echo pre","lint":"eslint ."}}"#,
            )],
        );
        let dev = planned(root, "dev", &["--port", "1 2"]);
        assert_eq!(
            dev.argv,
            [
                "cmd.exe",
                "/v:on",
                "/d",
                "/s",
                "/c",
                "cmd /d /s /c !DEVLANES_NPM_SCRIPT!"
            ]
        );
        assert_eq!(
            steps(&dev),
            [("DEVLANES_NPM_SCRIPT", r#""node server.js --port "1 2"""#)]
        );
        assert_eq!(
            dev.path[..2],
            [
                root.join(r"web\node_modules\.bin"),
                root.join(r"node_modules\.bin")
            ]
        );
        assert_eq!(dev.path.len(), root.join("web").ancestors().count());
        for (name, value) in [
            ("npm_package_name", "acme-web"),
            ("npm_package_version", "1.2.3"),
            ("npm_package_config_port", "8080"),
            ("npm_lifecycle_event", "dev"),
            ("npm_lifecycle_script", "node server.js"),
            ("npm_command", "run-script"),
        ] {
            assert_eq!(dev.env.get(name).map(String::as_str), Some(value), "{name}");
        }

        let build = planned(root, "build", &[]);
        assert_eq!(
            build.argv[5],
            "cmd /d /s /c !DEVLANES_NPM_PRE! && cmd /d /s /c !DEVLANES_NPM_SCRIPT! && cmd /d /s /c !DEVLANES_NPM_POST!"
        );
        assert_eq!(
            steps(&build),
            [
                ("DEVLANES_NPM_POST", r#""echo post""#),
                ("DEVLANES_NPM_PRE", r#""echo pre""#),
                ("DEVLANES_NPM_SCRIPT", r#""tsc""#),
            ]
        );
        let lint = planned(root, "lint", &[]);
        assert_eq!(
            lint.argv[5],
            "cmd /d /s /c !DEVLANES_NPM_PRE! && cmd /d /s /c !DEVLANES_NPM_SCRIPT!"
        );
        assert!(!lint.env.contains_key(POST));

        write(
            root,
            &[(
                r"web\.npmrc",
                "; shell\nscript-shell = C:\\Git\\usr\\bin\\bash.exe\n",
            )],
        );
        let bash = planned(root, "build", &["it's"]);
        assert_eq!(
            bash.argv,
            [
                r"C:\Git\usr\bin\bash.exe",
                "-c",
                r"(echo pre) && (tsc 'it'\''s') && (echo post)"
            ]
        );
        assert!(steps(&bash).is_empty());
    }

    /// Runs each composed line through the real `cmd.exe`, started with the
    /// C runtime's quoting as the runner starts it, in `<temp>\inner`, and
    /// compares what it prints and its exit code with npm's rule: each step
    /// in its own `cmd /d /s /c "<step>"`, stopping at the first that fails
    /// with that step's code.
    #[test]
    fn cmd_lines_run_steps_like_npm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let inner = temp.path().join("inner");
        std::fs::create_dir(&inner).expect("inner");
        let inner_path = inner.display().to_string();
        // `pre`, script, `post`, the lines printed and the exit code.
        type Row<'a> = (Option<&'a str>, &'a str, Option<&'a str>, Vec<&'a str>, i32);
        let rows: [Row<'_>; 12] = [
            // `cd` and `set` in `pre` stay in `pre`.
            (
                Some("cd .. && set ACME_X=1"),
                "cd && cmd /c echo [%ACME_X%]",
                Some("echo post"),
                vec![&inner_path, "[%ACME_X%]", "post"],
                0,
            ),
            (
                Some("cmd /c exit 3"),
                "echo b1 || echo b2",
                Some("echo post"),
                vec![],
                3,
            ),
            (
                Some("echo pre"),
                "cmd /c exit 4 || echo b2",
                Some("echo post"),
                vec!["pre", "b2", "post"],
                0,
            ),
            (
                Some("echo pre"),
                "cmd /c exit 5",
                Some("echo post"),
                vec!["pre"],
                5,
            ),
            (
                Some("echo pre"),
                "echo b1 & cmd /c exit 6",
                Some("echo post"),
                vec!["pre", "b1"],
                6,
            ),
            (
                Some("echo pre"),
                "echo body",
                Some("echo post | findstr post"),
                vec!["pre", "body", "post"],
                0,
            ),
            (
                Some("echo pre"),
                r#"echo (hi) a) %ACME_DIR% "a && b" bang! x"#,
                Some("echo post"),
                vec!["pre", r#"(hi) a) C:\Acme (x86) "a && b" bang! x"#, "post"],
                0,
            ),
            (
                Some("echo pre"),
                "if 1==2 (echo yes) else (echo no)",
                Some("echo post"),
                vec!["pre", "no", "post"],
                0,
            ),
            (
                Some("cmd /c exit 1 || echo p2"),
                "echo body",
                Some("echo post"),
                vec!["p2", "body", "post"],
                0,
            ),
            (
                None,
                r#"echo [%ACME_USER%] "a b" && cmd /c exit 7"#,
                None,
                vec![r#"[acme] "a b""#],
                7,
            ),
            (None, "echo a^&b ^| c", None, vec!["a&b | c"], 0),
            (
                None,
                "echo !DEVLANES_NPM_PRE! !PATH:~0,3!",
                None,
                vec!["!DEVLANES_NPM_PRE! !PATH:~0,3!"],
                0,
            ),
        ];
        for (pre, script, post, expected, code) in rows {
            let (line, steps) = cmd_steps(pre, script, post);
            let output = std::process::Command::new("cmd.exe")
                .args(["/v:on", "/d", "/s", "/c", &line])
                .envs(&steps)
                .env("ACME_DIR", r"C:\Acme (x86)")
                .env("ACME_USER", "acme")
                .env_remove("ACME_X")
                .current_dir(&inner)
                .output()
                .expect("cmd.exe");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
            assert_eq!(
                (lines, output.status.code()),
                (expected, Some(code)),
                "{pre:?} {script:?} {post:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
