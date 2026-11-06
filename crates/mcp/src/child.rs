//! Runs one devlanes command as a child process of the server, the way a shell
//! would, and collects its output.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// What one command printed.
#[derive(Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// The exit status; a child ended by a signal reads `128 + n`, as in a shell.
    pub exit_code: i32,
    /// What `--output json` would have printed: one document, an array for NDJSON,
    /// `Null` for nothing.
    pub data: Value,
    /// The call was cancelled and the child killed before it finished.
    pub cancelled: bool,
}

/// How to run the commands of one server.
#[derive(Debug, Clone)]
pub struct Runner {
    pub exe: PathBuf,
    /// The child's whole environment.
    pub env: BTreeMap<String, String>,
}

impl Runner {
    /// Runs `exe --output agent ARGS` in `cwd` until it exits or `cancel` fires,
    /// which kills it. With `from_lane` the child sends the daemon no env.
    /// Stdin is closed and stdout piped, so the child never touches the MCP stream.
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[String],
        from_lane: bool,
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        let json = tokio::task::spawn_blocking(|| {
            tempfile::Builder::new()
                .prefix("devlanes-mcp-")
                .suffix(".json")
                .tempfile()
                .map(tempfile::NamedTempFile::into_temp_path)
        })
        .await
        .map_err(std::io::Error::other)??;
        let mut command = self.command(cwd);
        command
            .args(["--output", "agent"])
            .args(args)
            .env("DEVLANES_JSON_OUT", &*json);
        if from_lane {
            command.env("DEVLANES_ENV_FROM_LANE", "1");
        }
        let mut output = collect(command, cancel).await?;
        let written = tokio::fs::read_to_string(&*json).await.unwrap_or_default();
        output.data = parse_json(&written);
        // The temporary file goes on a blocking thread too.
        let _ = tokio::task::spawn_blocking(move || json.close()).await;
        Ok(output)
    }

    /// Runs `exe --output json ARGS` and returns its parsed stdout, for the
    /// app-only tools.
    pub async fn json(
        &self,
        cwd: &Path,
        args: &[String],
        cancel: &CancellationToken,
    ) -> std::io::Result<Output> {
        let mut command = self.command(cwd);
        command.args(["--output", "json"]).args(args);
        let mut output = collect(command, cancel).await?;
        output.data = parse_json(&output.stdout);
        Ok(output)
    }

    /// A command for the devlanes executable with the server's environment.
    pub fn command(&self, cwd: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.exe);
        command
            .env_clear()
            .envs(&self.env)
            // Values the server itself was started with belong to its parent's call.
            .env_remove("DEVLANES_JSON_OUT")
            .env_remove("DEVLANES_ENV_FROM_LANE")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

/// Spawns `command` and reads its output until it exits or `cancel` fires.
async fn collect(
    mut command: tokio::process::Command,
    cancel: &CancellationToken,
) -> std::io::Result<Output> {
    let mut child = command.spawn()?;
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    let (status, cancelled) = tokio::select! {
        status = child.wait() => (Some(status?), false),
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            (None, true)
        }
    };
    let (stdout, stderr) = tokio::join!(stdout, stderr);
    Ok(Output {
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
        exit_code: status.map_or(130, exit_code),
        data: Value::Null,
        cancelled,
    })
}

/// Everything a pipe delivers, as text.
async fn read_all(
    pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
) -> Option<String> {
    let mut pipe = pipe?;
    let reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes).await;
        String::from_utf8_lossy(&bytes).into_owned()
    });
    reader.await.ok()
}

/// The code a shell would report for `status`.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

/// One JSON document as it is, NDJSON lines as an array, `Null` for no output.
/// Lines that are not JSON are skipped.
pub fn parse_json(text: &str) -> Value {
    let mut documents: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    match documents.len() {
        0 => Value::Null,
        1 => documents.remove(0),
        _ => Value::Array(documents),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A document stays as it is; a stream becomes an array.
    #[test]
    fn json_documents_and_streams() {
        assert_eq!(parse_json(""), Value::Null);
        assert_eq!(parse_json("{\"a\":1}\n"), json!({"a": 1}));
        assert_eq!(
            parse_json("{\"a\":1}\n{\"b\":2}\n"),
            json!([{"a": 1}, {"b": 2}])
        );
    }
}
