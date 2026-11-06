//! The skill and the MCP guide name commands and flags that exist.

use clap::{Parser, error::ErrorKind};
use devlanes_cli::Cli;

/// Every span between backticks in `text` that starts with `devlanes `.
fn quoted_commands(text: &str) -> Vec<&str> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.starts_with("devlanes "))
        .collect()
}

/// Catches commands in what agents read that a CLI change renamed or removed.
/// Usage forms such as `devlanes with [units...] -- COMMAND` are skipped, and
/// a bare `devlanes exec` naming the command without its arguments passes.
#[test]
fn every_command_agents_read_parses() {
    for file in ["skills/devlanes/SKILL.md", "docs/guides/mcp.md"] {
        let path = format!("{}/../../{file}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
        let commands = quoted_commands(&text);
        assert!(commands.len() > 5, "{file}: found {commands:?}");
        for command in commands
            .into_iter()
            .filter(|command| !command.contains('['))
        {
            match Cli::try_parse_from(command.split_whitespace()) {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::MissingRequiredArgument
                            | ErrorKind::DisplayHelp
                            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    ) => {}
                Err(error) => panic!("{file}: `{command}` does not parse: {error}"),
            }
        }
    }
}
