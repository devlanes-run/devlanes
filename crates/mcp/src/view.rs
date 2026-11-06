//! Which MCP Apps card shows a tool result (`structuredContent.view`).

use serde_json::Value;

/// The card for the result of the command at `path` (`lane clean`), or `None`
/// for results without one.
///
/// - `start`, `restart`, `reload`: `lane`, or `failure` when a unit failed or
///   the command did;
/// - `status`: `lane` for one lane, `board` otherwise;
/// - `debug why`, `debug errors`, `logs`: `failure`;
/// - `lane clean`: `clean`; `daemon pair`: `pair`.
pub fn select(path: &str, exit_code: i32, data: &Value) -> Option<&'static str> {
    let failed = || {
        exit_code != 0
            || data.get("error").is_some()
            || data
                .get("failures")
                .and_then(Value::as_array)
                .is_some_and(|failures| !failures.is_empty())
    };
    match path {
        "start" | "restart" | "reload" => Some(if failed() { "failure" } else { "lane" }),
        "status" if data.get("error").is_some() => None,
        "status" => Some(
            match data.get("items").and_then(Value::as_array).map(Vec::len) {
                Some(1) => "lane",
                _ => "board",
            },
        ),
        "debug why" | "debug errors" | "logs" => Some("failure"),
        "lane clean" => Some("clean"),
        "daemon pair" => Some("pair"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The rules of the tool-result contract.
    #[test]
    fn views() {
        let one = json!({"items": [{"reference": "acme@1"}]});
        let two = json!({"items": [{}, {}]});
        assert_eq!(select("status", 0, &one), Some("lane"));
        assert_eq!(select("status", 0, &two), Some("board"));
        assert_eq!(select("status", 1, &json!({"error": {}})), None);
        assert_eq!(select("start", 0, &json!({"failures": []})), Some("lane"));
        assert_eq!(
            select("start", 1, &json!({"failures": [{"unit": "api"}]})),
            Some("failure")
        );
        assert_eq!(select("restart", 4, &Value::Null), Some("failure"));
        assert_eq!(select("logs", 0, &Value::Null), Some("failure"));
        assert_eq!(select("debug why", 0, &Value::Null), Some("failure"));
        assert_eq!(select("lane clean", 0, &Value::Null), Some("clean"));
        assert_eq!(select("daemon pair", 0, &Value::Null), Some("pair"));
        assert_eq!(select("stop", 0, &Value::Null), None);
    }
}
