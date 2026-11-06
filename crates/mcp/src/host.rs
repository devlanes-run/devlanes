//! What the MCP client's name says about the host: how long it waits for a
//! tool call, and which agent it is when the environment does not say.
//!
//! The `clientInfo.name` each host sends, and where it was checked:
//!
//! - Claude Code: `claude-code` (the `Client` it builds in the 2.1.318 binary).
//! - Codex: `codex-mcp-client` (`codex-rs/codex-mcp/src/rmcp_client.rs`); some
//!   Codex surfaces send `Codex` (canimcp.dev).
//! - OpenCode: `opencode` (`packages/core/src/mcp/client.ts`).
//! - Cursor: `cursor-vscode` (MCP logs on forum.cursor.com, canimcp.dev).
//! - VS Code: `Visual Studio Code`, its product `nameLong`
//!   (`src/vs/workbench/contrib/mcp/common/mcpServerRequestHandler.ts`);
//!   Insiders adds ` - Insiders`.
//! - GitHub Copilot CLI: `github-copilot-developer` (canimcp.dev).
//! - Gemini CLI: `gemini-cli-mcp-client` (`packages/core/src/tools/mcp-client.ts`).
//! - Goose: `goose-cli` or `goose-desktop`, its platform name
//!   (`crates/goose/src/agents/services.rs`).
//! - Claude Desktop: `claude-ai` (canimcp.dev).

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

/// The `AI_AGENT` value is-ai-agent 0.7 knows for this client (`claude-code`,
/// `codex`), or `None` for a client that is not a coding agent it knows, such
/// as Claude Desktop.
pub fn agent_id(client: &str) -> Option<&'static str> {
    let client = client.to_ascii_lowercase();
    // First match wins: Cursor's `cursor-vscode` must not reach a VS Code entry.
    [
        ("claude-code", "claude-code"),
        ("codex", "codex"),
        ("opencode", "opencode"),
        ("cursor", "cursor"),
        ("gemini", "gemini-cli"),
        ("goose", "goose"),
        ("copilot", "github-copilot"),
        // VS Code runs MCP servers for Copilot's agent mode.
        ("visual studio code", "github-copilot"),
    ]
    .into_iter()
    .find(|(name, _)| client.contains(name))
    .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts the timeout and agent id for a `clientInfo.name`.
    fn host(client: &str, timeout: u64, agent: Option<&str>) {
        assert_eq!(
            default_timeout(client),
            Duration::from_secs(timeout),
            "{client}"
        );
        assert_eq!(agent_id(client), agent, "{client}");
    }

    #[test]
    fn claude_code() {
        host("claude-code", 170, Some("claude-code"));
    }

    #[test]
    fn codex() {
        host("codex-mcp-client", 50, Some("codex"));
        host("Codex", 50, Some("codex"));
    }

    #[test]
    fn opencode() {
        host("opencode", 25, Some("opencode"));
    }

    #[test]
    fn cursor() {
        host("cursor-vscode", 25, Some("cursor"));
        host("cursor-vscode (via mcp-remote 0.1.37)", 25, Some("cursor"));
    }

    #[test]
    fn vs_code_copilot() {
        host("Visual Studio Code", 25, Some("github-copilot"));
        host("Visual Studio Code - Insiders", 25, Some("github-copilot"));
        host("github-copilot-developer", 25, Some("github-copilot"));
    }

    #[test]
    fn gemini_cli() {
        host("gemini-cli-mcp-client", 25, Some("gemini-cli"));
    }

    #[test]
    fn goose() {
        host("goose-cli", 25, Some("goose"));
        host("goose-desktop", 25, Some("goose"));
    }

    #[test]
    fn claude_desktop() {
        host("claude-ai", 25, None);
    }

    #[test]
    fn unknown() {
        host("", 25, None);
        host("acme-client", 25, None);
    }

    /// Every id the table sets is one is-ai-agent 0.7 classifies as that agent,
    /// so the child's actor is the named agent and not `unknown`.
    #[test]
    fn agent_ids_are_known_to_is_ai_agent() {
        for client in [
            "claude-code",
            "codex-mcp-client",
            "opencode",
            "cursor-vscode",
            "gemini-cli-mcp-client",
            "goose-cli",
            "github-copilot-developer",
            "Visual Studio Code",
        ] {
            let id = agent_id(client).expect("an agent id");
            let agent = is_ai_agent::detect_with(
                |name| (name == "AI_AGENT").then(|| id.to_owned()),
                |_| false,
            )
            .expect("detected");
            assert_eq!(agent.id.as_str(), id, "{client}");
        }
    }
}
