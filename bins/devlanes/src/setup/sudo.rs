//! sudo for the `hosts` and `resolver` steps: prompts only where a person can
//! answer, and one `sudo -v` up front so the steps reuse its timestamp.

use std::path::Path;

use async_trait::async_trait;
use devlanes_cli::Runtime;
use devlanes_proto::RpcError;
use devlanes_resolver_hosts::{CommandOutput, CommandSpec, Executor, SystemExecutor};
use serde_json::json;

/// The executor for privileged steps. sudo may prompt in an interactive terminal;
/// agents and CI get `sudo -n` on every step, and only when it already works.
/// `command` is what the user should run from a terminal instead.
pub(super) async fn privileged(
    runtime: &Runtime,
    command: &str,
) -> Result<Box<dyn Executor>, RpcError> {
    if runtime.can_prompt() {
        return Ok(Box::new(SystemExecutor));
    }
    let probe = CommandSpec::new("sudo").args(["-n", "true"]);
    let works = SystemExecutor
        .run(&probe)
        .await
        .is_ok_and(|output| output.success());
    if !works {
        return Err(RpcError::new(
            "terminal_required",
            json!({
                "command": command,
                "reason": "sudo needs a password and this session cannot prompt",
            }),
            2,
        ));
    }
    Ok(Box::new(NonInteractive(SystemExecutor)))
}

/// Asks for the password once with `sudo -v`, in this terminal, before the
/// steps run; root and sessions that cannot prompt skip it.
pub(super) async fn refresh(runtime: &Runtime) -> Result<(), RpcError> {
    if !runtime.can_prompt() || rustix::process::geteuid().is_root() {
        return Ok(());
    }
    let sudo = super::resolver::system_program("sudo")?;
    let status = tokio::process::Command::new(&sudo)
        .arg("-v")
        .status()
        .await
        .map_err(|error| RpcError::new("io", json!({"message": format!("{sudo}: {error}")}), 1))?;
    if status.success() {
        return Ok(());
    }
    Err(RpcError::new(
        "sudo_failed",
        json!({"message": "sudo -v failed, so nothing was changed"}),
        1,
    ))
}

/// Runs every `sudo` as `sudo -n`, so a session that cannot answer a prompt fails
/// instead of waiting on one when the sudo timestamp lapses mid-install.
struct NonInteractive<E>(E);

#[async_trait]
impl<E: Executor> Executor for NonInteractive<E> {
    /// Adds `-n` to sudo commands and runs them with the inner executor.
    async fn run(&self, command: &CommandSpec) -> std::io::Result<CommandOutput> {
        self.0.run(&non_interactive(command)).await
    }
}

/// `sudo X` (or `/usr/bin/sudo X`) becomes `sudo -n X`; other commands are unchanged.
fn non_interactive(command: &CommandSpec) -> CommandSpec {
    let mut command = command.clone();
    let sudo = Path::new(&command.program)
        .file_name()
        .is_some_and(|name| name == "sudo");
    if sudo && command.args.first().map(String::as_str) != Some("-n") {
        command.args.insert(0, "-n".into());
    }
    command
}

#[cfg(test)]
mod tests {
    use devlanes_resolver_hosts::INSTALL_PATH;

    use super::*;

    #[test]
    fn non_interactive_sessions_never_let_sudo_prompt() {
        let step = CommandSpec::new("sudo").args(["/bin/mv", "-f", "a", "b"]);
        assert_eq!(non_interactive(&step).to_string(), "sudo -n /bin/mv -f a b");
        let already = CommandSpec::new("sudo").args(["-n", "-k", INSTALL_PATH, "version"]);
        assert_eq!(non_interactive(&already), already);
        let plain = CommandSpec::new("/usr/bin/id").arg("-un");
        assert_eq!(non_interactive(&plain), plain);
        let absolute = CommandSpec::new("/usr/bin/sudo").args(["/bin/rm", "-f", "x"]);
        assert_eq!(
            non_interactive(&absolute).to_string(),
            "/usr/bin/sudo -n /bin/rm -f x"
        );
    }
}
