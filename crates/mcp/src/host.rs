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

use std::{collections::BTreeMap, time::Duration};

/// Claude Code's limit for every MCP tool call, in milliseconds
/// (code.claude.com/docs/en/mcp, "Time limits on MCP tool calls"). Its servers
/// inherit Claude Code's environment, so a value set there reaches this one.
const CLAUDE_CODE_TOOL_TIMEOUT: &str = "MCP_TOOL_TIMEOUT";

/// The `--timeout` added to blocking commands that have none: a little under
/// what the host waits for a tool call before it gives up on it.
///
/// Codex's documented `tool_timeout_sec` default is 60 s
/// (developers.openai.com/codex/extend/mcp), so its 50 s holds even where newer
/// builds wait longer. OpenCode waits 30 s. Claude Code waits about 28 hours
/// unless `MCP_TOOL_TIMEOUT` says otherwise; without it the CLI's own 180 s
/// default would do, and 170 s keeps the result in the call rather than in a
/// background task for longer. Other hosts get 60 s, the default request
/// timeout of the MCP TypeScript SDK many clients are built on.
pub fn default_timeout(client: &str, env: &BTreeMap<String, String>) -> Duration {
    let client = client.to_ascii_lowercase();
    if client.contains("claude-code") {
        return claude_code_limit(env)
            .map(under)
            .unwrap_or(Duration::from_secs(170));
    }
    Duration::from_secs(if client.contains("codex") {
        50
    } else if client.contains("opencode") {
        25
    } else {
        60
    })
}

/// `MCP_TOOL_TIMEOUT` as Claude Code reads it: whole milliseconds, where values
/// below 1000 are ignored.
fn claude_code_limit(env: &BTreeMap<String, String>) -> Option<Duration> {
    env.get(CLAUDE_CODE_TOOL_TIMEOUT)?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms >= 1000)
        .map(Duration::from_millis)
}

/// A little under `limit`, so devlanes answers before the host gives up: a
/// tenth of it, between 1 s and 10 s, and never below 1 s in total.
fn under(limit: Duration) -> Duration {
    let margin = (limit / 10).clamp(Duration::from_secs(1), Duration::from_secs(10));
    limit.saturating_sub(margin).max(Duration::from_secs(1))
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
            default_timeout(client, &BTreeMap::new()),
            Duration::from_secs(timeout),
            "{client}"
        );
        assert_eq!(agent_id(client), agent, "{client}");
    }

    #[test]
    fn claude_code() {
        host("claude-code", 170, Some("claude-code"));
    }

    /// Claude Code's `MCP_TOOL_TIMEOUT` sets the limit; values Claude Code
    /// ignores (below 1000 ms, not a number) leave the default.
    #[test]
    fn claude_code_tool_timeout() {
        let limit = |value: &str| {
            let env = BTreeMap::from([(CLAUDE_CODE_TOOL_TIMEOUT.to_owned(), value.to_owned())]);
            default_timeout("claude-code", &env)
        };
        assert_eq!(limit("600000"), Duration::from_secs(590));
        assert_eq!(limit("60000"), Duration::from_secs(54));
        assert_eq!(limit("5000"), Duration::from_secs(4));
        assert_eq!(limit("1000"), Duration::from_secs(1));
        assert_eq!(limit("999"), Duration::from_secs(170));
        assert_eq!(limit("ten minutes"), Duration::from_secs(170));
        // Only Claude Code reads the variable.
        let env = BTreeMap::from([(CLAUDE_CODE_TOOL_TIMEOUT.to_owned(), "600000".to_owned())]);
        assert_eq!(
            default_timeout("codex-mcp-client", &env),
            Duration::from_secs(50)
        );
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
        host("cursor-vscode", 60, Some("cursor"));
        host("cursor-vscode (via mcp-remote 0.1.37)", 60, Some("cursor"));
    }

    #[test]
    fn vs_code_copilot() {
        host("Visual Studio Code", 60, Some("github-copilot"));
        host("Visual Studio Code - Insiders", 60, Some("github-copilot"));
        host("github-copilot-developer", 60, Some("github-copilot"));
    }

    #[test]
    fn gemini_cli() {
        host("gemini-cli-mcp-client", 60, Some("gemini-cli"));
    }

    #[test]
    fn goose() {
        host("goose-cli", 60, Some("goose"));
        host("goose-desktop", 60, Some("goose"));
    }

    #[test]
    fn claude_desktop() {
        host("claude-ai", 60, None);
    }

    #[test]
    fn unknown() {
        host("", 60, None);
        host("acme-client", 60, None);
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
