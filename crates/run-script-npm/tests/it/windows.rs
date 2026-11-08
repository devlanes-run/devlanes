//! npm scripts planned by `NpmScripts` and started by the Windows runner,
//! which runs them without npm.

use std::{collections::BTreeMap, ffi::OsString, path::Path, sync::Arc, time::Duration};

use devlanes_core::{
    ExitOutcome, LaunchSpec, OutputChunk, OutputStream, StopPolicy, StopSignal, TerminalSize, Unit,
    contrib::{self, ScriptPlan, ScriptRunner},
};
use devlanes_run_script_npm::NpmScripts;
use devlanes_runner_windows::{
    ConsoleHelper, HELPER_ARG, WindowsRunner, WindowsUnit, console_ctrl_main,
};
use tokio::sync::broadcast::{Receiver, error::RecvError};

use super::{COMMAND_LIMIT, QUIET, write_files};

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

/// A pipe unit for `plan`: this process's environment with the plan's
/// variables over it. A hand-written stand-in for lane resolution that keeps
/// only what the stop test needs (`node` is already on `PATH`), so tests
/// using it prove the plan and the runner, not `resolve`'s layering.
fn spec(plan: ScriptPlan) -> LaunchSpec {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
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

/// Every chunk until the channel closes.
async fn drain(mut output: Receiver<OutputChunk>) -> Vec<OutputChunk> {
    let mut chunks = Vec::new();
    loop {
        match output.recv().await {
            Ok(chunk) => chunks.push(chunk),
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return chunks,
        }
    }
}

/// Runs `argv` in `dir` as a pipe unit, the way the daemon starts a package
/// manager, within [`COMMAND_LIMIT`]. Returns whether it exited 0 and its
/// stdout, or `None` when the runner could not start it.
pub(super) fn run(argv: &[String], dir: &Path) -> Option<(bool, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.extend(QUIET.map(|(name, value)| (name.to_owned(), value.to_owned())));
        let unit = runner()
            .spawn_unit(LaunchSpec {
                program: argv[0].clone(),
                args: argv[1..].to_vec(),
                cwd: dir.to_path_buf(),
                env,
                pty: false,
                size: TerminalSize::default(),
            })
            .await
            .ok()?;
        let _cleanup = Cleanup(unit.clone());
        let output = unit.output();
        tokio::time::timeout(COMMAND_LIMIT, async {
            let chunks = drain(output).await;
            let outcome = unit.wait().await.expect("wait");
            let mut stdout = String::new();
            for chunk in chunks {
                let text = String::from_utf8_lossy(&chunk.bytes);
                match chunk.stream {
                    OutputStream::Stderr => eprint!("{text}"),
                    _ => stdout.push_str(&text),
                }
            }
            Some((outcome.code == Some(0), stdout))
        })
        .await
        .unwrap_or_else(|_| panic!("{argv:?} timed out"))
    })
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
