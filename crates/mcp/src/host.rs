//! What the MCP client's name says about the host: how long it waits for a
//! tool call, and which agent it is when the environment does not say.

use std::time::Duration;

/// The `--timeout` added to blocking commands that have none: a little under
/// what the host waits for a tool call before it gives up on it.
pub fn default_timeout(client: &str) -> Duration {
    let client = client.to_ascii_lowercase();
    Duration::from_secs(if client.contains("claude-code") {
        170
    } else if client.contains("codex") {
        50
    } else {
        // OpenCode waits 30 s by default; unknown hosts get the same margin.
        25
    })
}

/// The `AI_AGENT` value is-ai-agent knows for this client (`claude-code`,
/// `codex`), or `None` for a client it would not recognize, such as Claude
/// Desktop.
pub fn agent_id(client: &str) -> Option<&'static str> {
    let client = client.to_ascii_lowercase();
    [
        ("claude-code", "claude-code"),
        ("codex", "codex"),
        ("opencode", "opencode"),
        ("cursor", "cursor"),
        ("gemini", "gemini-cli"),
        ("goose", "goose"),
        ("copilot", "github-copilot"),
        ("visual studio code", "github-copilot"),
        ("vscode", "github-copilot"),
    ]
    .into_iter()
    .find(|(name, _)| client.contains(name))
    .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client names as the hosts send them in `clientInfo.name`.
    #[test]
    fn hosts_by_client_name() {
        assert_eq!(default_timeout("claude-code"), Duration::from_secs(170));
        assert_eq!(default_timeout("codex-mcp-client"), Duration::from_secs(50));
        assert_eq!(default_timeout("opencode"), Duration::from_secs(25));
        assert_eq!(default_timeout(""), Duration::from_secs(25));
        assert_eq!(agent_id("claude-code"), Some("claude-code"));
        assert_eq!(agent_id("codex-mcp-client"), Some("codex"));
        assert_eq!(agent_id("cursor-vscode"), Some("cursor"));
        assert_eq!(agent_id("Visual Studio Code"), Some("github-copilot"));
        assert_eq!(agent_id("claude-ai"), None);
    }
}
