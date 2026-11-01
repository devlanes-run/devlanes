//! `devlanes uninstall` of a copy of the binary in its own directory, which
//! stands in for an `install.sh` or `install.ps1` install, with the lab's
//! home. Without a terminal the home stays unless `--remove-data` is given.

#[cfg(windows)]
use std::time::Duration;
use std::{env::consts::EXE_SUFFIX, path::Path};

use devlanes_e2e::{COMMAND_TIMEOUT, Lab, Result, bin_dir};
use serde_json::Value;

use crate::{BUDGET, need};

/// Bound for the detached deletion of `devlanes.exe` after the command exits.
#[cfg(windows)]
const DELETE_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
fn uninstall_removes_a_script_install_and_keeps_the_home() -> Result<()> {
    // Dropped after the lab, so a failed run leaves nothing behind.
    let root = tempfile::tempdir()?;
    let lab = Lab::without_config(BUDGET)?;
    let install = root.path().join("bin");
    std::fs::create_dir_all(&install)?;
    let entry = install.join(format!("devlanes{EXE_SUFFIX}"));
    std::fs::copy(bin_dir()?.join(format!("devlanes{EXE_SUFFIX}")), &entry)?;
    #[cfg(unix)]
    std::fs::write(install.join("devlanes-hosts"), "helper")?;

    let plan = lab
        .run(
            lab.command_with(&entry, lab.repo(), &["uninstall", "--dry-run", "--json"])?,
            COMMAND_TIMEOUT,
        )?
        .expect_code(0)?
        .json()?;
    assert_eq!(plan["dry_run"], true, "{plan}");
    let binary = need(step(&plan, "binary"), "the binary step")?;
    assert_eq!(binary["state"], "todo", "{plan}");
    let deletes = binary["detail"][0].as_str().unwrap_or_default();
    let deletes = if cfg!(windows) {
        deletes.to_lowercase()
    } else {
        deletes.to_owned()
    };
    assert!(
        deletes.starts_with("delete ") && deletes.contains(&dunce_like(&entry)),
        "{plan}"
    );
    assert_eq!(
        need(step(&plan, "daemon"), "the daemon step")?["state"],
        "skip"
    );
    // No terminal: the data stays, and the plan lists the home.
    assert_eq!(plan["data"]["action"], "keep", "{plan}");
    let home = plan["data"]["items"][0]["path"]
        .as_str()
        .unwrap_or_default();
    assert!(same(Path::new(home), lab.home()), "{plan}");
    assert!(entry.is_file(), "the dry run deleted nothing");

    // Undoing `devlanes setup` changes the machine, not the lab; a machine
    // set up for real keeps its setup, and the rest of the test is skipped.
    let setup = need(step(&plan, "setup"), "the setup step")?;
    if setup["state"] != "skip" {
        eprintln!("skipped: this machine has devlanes setup changes: {setup}");
        return Ok(());
    }

    let output = lab.run(
        lab.command_with(
            &entry,
            lab.repo(),
            &["uninstall", "--keep-data", "--yes", "--json"],
        )?,
        COMMAND_TIMEOUT,
    )?;
    let result = output.expect_code(0)?.json()?;
    assert_eq!(result["dry_run"], false, "{output}");
    assert_eq!(
        need(step(&result, "binary"), "the binary step")?["state"],
        "done",
        "{output}"
    );
    assert_eq!(result["data"]["deleted"], false, "{output}");
    #[cfg(unix)]
    {
        assert!(!entry.exists(), "{output}");
        assert!(!install.join("devlanes-hosts").exists(), "{output}");
    }
    // A running executable cannot delete itself; a detached cmd.exe deletes
    // it once the command has exited.
    #[cfg(windows)]
    lab.eventually("devlanes.exe to be deleted", DELETE_TIMEOUT, || {
        Ok((!entry.exists()).then_some(()))
    })?;
    assert!(lab.home().join("config.yaml").is_file(), "the home stays");
    Ok(())
}

/// The step named `name` of an uninstall plan or result.
fn step(report: &Value, name: &str) -> Option<Value> {
    report["steps"]
        .as_array()?
        .iter()
        .find(|step| step["step"] == name)
        .cloned()
}

/// Whether two paths name the same directory.
fn same(a: &Path, b: &Path) -> bool {
    dunce_like(a) == dunce_like(b)
}

/// `path` canonical, without Windows' `\\?\` prefix, compared without case on Windows.
fn dunce_like(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = path.display().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}
