//! npm scripts planned by `NpmScripts` and started by the Windows runner,
//! which runs them without npm.

use std::{collections::BTreeMap, ffi::OsString, path::Path, sync::Arc, time::Duration};

use devlanes_core::{
    ExitOutcome, LaunchSpec, OutputChunk, StopPolicy, StopSignal, TerminalSize, Unit,
    contrib::{self, ScriptPlan, ScriptRunner},
};
use devlanes_run_script_npm::NpmScripts;
use devlanes_runner_windows::{
    ConsoleHelper, HELPER_ARG, WindowsRunner, WindowsUnit, console_ctrl_main,
};
use tokio::sync::broadcast::{Receiver, error::RecvError};

use super::write_files;

/// Upper bound for each test body.
const LIMIT: Duration = Duration::from_secs(60);

/// Not a test of its own: started with [`HELPER_ARG`], this binary is the
/// runner's console helper, as `devlanes __console-ctrl` is in the shipped
/// binary.
#[test]
fn helper_entry() {
    let args: Vec<OsString> = std::env::args_os().collect();
    if let Some(at) = args.iter().position(|arg| arg == HELPER_ARG) {
        let code = console_ctrl_main(args[at + 1..].iter().cloned());
        std::process::exit(i32::from(code));
    }
}

/// The runner with this test binary as its console helper.
fn runner() -> WindowsRunner {
    let helper = ConsoleHelper {
        program: std::env::current_exe().expect("test binary path"),
        args: [
            "--exact",
            "windows::helper_entry",
            "--nocapture",
            HELPER_ARG,
        ]
        .map(OsString::from)
        .into(),
    };
    WindowsRunner::default().with_console_helper(helper)
}

/// Plans script `name` of `<root>/package.json` through the core adapter.
async fn script(root: &Path, name: &str) -> ScriptPlan {
    NpmScripts
        .resolve(&contrib::ScriptRequest {
            worktree: root,
            source: Path::new("package.json"),
            name,
            extra_args: &[],
        })
        .await
        .expect("plan")
}

/// A pipe unit for `plan` with this process's environment under the plan's
/// variables and the plan's directories in front of `PATH`, as lane
/// resolution layers them.
fn spec(plan: ScriptPlan) -> LaunchSpec {
    let mut env: BTreeMap<String, String> = std::env::vars()
        .map(|(name, value)| (name.to_uppercase(), value))
        .collect();
    let path = std::env::join_paths(
        plan.path
            .iter()
            .cloned()
            .chain(std::env::split_paths(&env["PATH"])),
    )
    .expect("PATH");
    env.insert("PATH".into(), path.to_string_lossy().into_owned());
    env.extend(plan.env);
    LaunchSpec {
        program: plan.argv[0].clone(),
        args: plan.argv[1..].to_vec(),
        cwd: plan.cwd,
        env,
        pty: false,
        size: TerminalSize::default(),
    }
}

/// Ends the unit's job when the test is done, passed or failed.
struct Cleanup(Arc<WindowsUnit>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.0.terminate();
    }
}

/// Everything the unit printed until its output closed.
async fn drain(mut output: Receiver<OutputChunk>) -> String {
    let mut text = String::new();
    loop {
        match output.recv().await {
            Ok(chunk) => text.push_str(&String::from_utf8_lossy(&chunk.bytes)),
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return text,
        }
    }
}

/// A failing `pre` step ends the script with its code before the script
/// itself runs, as npm does, through the plan's step variables and the
/// runner's quoting of the `cmd.exe` line.
#[tokio::test]
async fn npm_steps_stop_at_a_failing_pre_script() {
    tokio::time::timeout(LIMIT, async {
        let temp = tempfile::tempdir().expect("tempdir");
        write_files(
            temp.path(),
            &[(
                "package.json",
                r#"{"scripts":{"prego":"echo PRE-RAN && exit 1","go":"echo BODY-RAN","postgo":"echo POST-RAN"}}"#,
            )],
        );
        let unit = runner()
            .spawn_unit(spec(script(temp.path(), "go").await))
            .await
            .expect("spawn");
        let _cleanup = Cleanup(unit.clone());
        let output = drain(unit.output()).await;
        let outcome = unit.wait().await.expect("exit");
        assert_eq!(outcome, ExitOutcome { code: Some(1), signal: None }, "{output}");
        assert!(output.contains("PRE-RAN"), "{output}");
        assert!(!output.contains("BODY-RAN") && !output.contains("POST-RAN"), "{output}");
    })
    .await
    .expect("bounded");
}

/// A stopped npm script reports its program's own exit code: npm is not
/// there to turn the stop into exit code 1.
#[tokio::test]
#[ignore = "needs node"]
async fn npm_scripts_report_their_own_code_after_a_stop() {
    devlanes_sys_windows::console::set_ctrl_c_ignored(false).expect("accept Ctrl-C");
    tokio::time::timeout(LIMIT, async {
        let temp = tempfile::tempdir().expect("tempdir");
        write_files(
            temp.path(),
            &[
                (
                    "server.js",
                    "process.on('SIGINT', () => process.exit(0));\n\
                     require('http').createServer().listen(0, '127.0.0.1', () => console.log('LISTENING'));\n",
                ),
                ("package.json", r#"{"scripts":{"dev":"node server.js"}}"#),
            ],
        );
        let unit = runner()
            .spawn_unit(spec(script(temp.path(), "dev").await))
            .await
            .expect("spawn");
        let _cleanup = Cleanup(unit.clone());
        let mut output = unit.output();
        let mut seen = String::new();
        while !seen.contains("LISTENING") {
            match output.recv().await {
                Ok(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk.bytes)),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("output ended: {seen}"),
            }
        }
        let report = unit
            .stop_with_report(StopPolicy {
                signal: StopSignal::Terminate,
                timeout: Duration::from_secs(20),
            })
            .await
            .expect("stop");
        assert!(report.stop.is_some_and(|stop| !stop.escalated), "{report:?}");
        assert_eq!(
            report.outcome,
            ExitOutcome {
                code: Some(0),
                signal: None
            },
            "{report:?}"
        );
    })
    .await
    .expect("bounded");
}
