//! Which MCP Apps card shows a tool result (`structuredContent.view`), and the
//! number of each lane's cards (`structuredContent.card`).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex, PoisonError},
};

use serde_json::Value;

use crate::command::Call;

/// The card for the result of `call`, or `None` for results without one.
///
/// - `start`, `restart`, `reload`: `lane`, or `failure` when a unit failed or
///   the command did;
/// - `status`: `lane` for one lane, `board` otherwise;
/// - `debug why`, `debug errors`, `logs`: `failure`, except for logs an agent
///   reads with a query ([`Call::log_query`]), which the text already shows;
/// - `lane clean`: `clean`; `daemon pair`: `pair`; `debug top`: `top`.
pub fn select(call: &Call, exit_code: i32, data: &Value) -> Option<&'static str> {
    let failed = || {
        exit_code != 0
            || data.get("error").is_some()
            || data
                .get("failures")
                .and_then(Value::as_array)
                .is_some_and(|failures| !failures.is_empty())
    };
    match call.path.as_str() {
        "start" | "restart" | "reload" => Some(if failed() { "failure" } else { "lane" }),
        "status" if data.get("error").is_some() => None,
        "status" => Some(
            match data.get("items").and_then(Value::as_array).map(Vec::len) {
                Some(1) => "lane",
                _ => "board",
            },
        ),
        "logs" if call.log_query() => None,
        "debug why" | "debug errors" | "logs" => Some("failure"),
        "lane clean" => Some("clean"),
        "daemon pair" => Some("pair"),
        "debug top" => Some("top"),
        _ => None,
    }
}

/// The lane `data` is about when it names exactly one: a document's `lane`
/// (start results, `debug why`, clean plans), and the `reference` or `lane`
/// of its `items` (status pages, units, `debug errors`) and `failures`. An
/// array, the lines of a stream, counts every document in it.
pub fn lane_of(data: &Value) -> Option<&str> {
    let mut lanes = BTreeSet::new();
    named(data, &mut lanes);
    let mut lanes = lanes.into_iter();
    match (lanes.next(), lanes.next()) {
        (Some(lane), None) => Some(lane),
        _ => None,
    }
}

/// Adds the lanes `document` names to `lanes`.
fn named<'a>(document: &'a Value, lanes: &mut BTreeSet<&'a str>) {
    if let Value::Array(documents) = document {
        for document in documents {
            named(document, lanes);
        }
        return;
    }
    lanes.extend(document.get("lane").and_then(Value::as_str));
    for list in ["items", "failures"] {
        for item in document
            .get(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            lanes.extend(
                item.get("reference")
                    .or_else(|| item.get("lane"))
                    .and_then(Value::as_str),
            );
        }
    }
}

/// The cards this server process gave each lane so far, so a card can tell
/// whether a newer one shows the same lane. Cheap to clone; clones share the
/// numbers, and concurrent calls each get their own.
#[derive(Clone, Default)]
pub struct Cards(Arc<Mutex<HashMap<String, u64>>>);

impl Cards {
    /// The number of a new card for `lane`: 1 for its first, then one more
    /// each time.
    pub fn next(&self, lane: &str) -> u64 {
        // A panic while the map was held leaves the numbers as they were.
        let mut cards = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let number = cards.entry(lane.to_owned()).or_default();
        *number += 1;
        *number
    }

    /// The newest card number of each of `lanes` that has had a card.
    pub fn newest<'a>(&self, lanes: impl Iterator<Item = &'a str>) -> BTreeMap<String, u64> {
        let cards = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lanes
            .filter_map(|lane| Some((lane.to_owned(), *cards.get(lane)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::command::Tool;

    /// The parsed `devlanes LINE`.
    fn call(line: &str) -> Call {
        let args: Vec<String> = line.split(' ').map(str::to_owned).collect();
        match Call::parse(&args, Tool::Any) {
            Ok(call) => call,
            Err(_) => panic!("devlanes {line} does not parse"),
        }
    }

    /// The rules of the tool-result contract.
    #[test]
    fn views() {
        let select = |line, exit_code, data: &Value| select(&call(line), exit_code, data);
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
        assert_eq!(select("logs api", 0, &Value::Null), Some("failure"));
        assert_eq!(
            select("logs api --tail 20", 0, &Value::Null),
            Some("failure")
        );
        assert_eq!(select("debug why api", 0, &Value::Null), Some("failure"));
        assert_eq!(select("lane clean acme@4", 0, &Value::Null), Some("clean"));
        assert_eq!(select("daemon pair", 0, &Value::Null), Some("pair"));
        assert_eq!(select("debug top", 0, &json!({"lanes": []})), Some("top"));
        assert_eq!(select("stop", 0, &Value::Null), None);
    }

    /// Logs an agent reads with a query get no card.
    #[test]
    fn log_queries_get_no_card() {
        for line in [
            "logs api --around api#120",
            "logs api --grep error",
            "logs api --cursor api#2",
            "logs api --since 5m",
            "logs api --head 10",
            "logs api --level warn",
            "logs api --follow --timeout 5s",
            "logs api -f --timeout 5s",
        ] {
            assert_eq!(select(&call(line), 0, &Value::Null), None, "{line}");
        }
    }

    /// Only data about one lane names it.
    #[test]
    fn the_lane_of_a_result() {
        let start = json!({"lane": "acme@1", "units": [{"lane": "acme@1"}], "failures": [{"lane": "acme@1"}]});
        assert_eq!(lane_of(&start), Some("acme@1"));
        assert_eq!(
            lane_of(&json!({"items": [{"reference": "acme@2"}]})),
            Some("acme@2")
        );
        assert_eq!(
            lane_of(
                &json!({"items": [{"lane": "acme@3", "unit": "api"}, {"lane": "acme@3", "unit": "web"}]})
            ),
            Some("acme@3")
        );
        assert_eq!(
            lane_of(&json!([{"lane": "acme@1"}, {"event": "with.summary"}])),
            Some("acme@1")
        );
        let board = json!({"items": [{"reference": "acme@1"}, {"reference": "acme@2"}]});
        assert_eq!(lane_of(&board), None);
        assert_eq!(
            lane_of(&json!({"items": [{"unit": "api", "seq": 1}]})),
            None
        );
        assert_eq!(lane_of(&json!({"code": "123-456"})), None);
        assert_eq!(lane_of(&Value::Null), None);
    }

    /// Numbers count up per lane, and clones share them.
    #[test]
    fn cards_count_per_lane() {
        let cards = Cards::default();
        assert_eq!(cards.next("acme@1"), 1);
        assert_eq!(cards.clone().next("acme@1"), 2);
        assert_eq!(cards.next("acme@2"), 1);
        assert_eq!(
            cards.newest(["acme@1", "acme@2", "acme@3"].into_iter()),
            BTreeMap::from([("acme@1".to_owned(), 2), ("acme@2".to_owned(), 1)])
        );
    }
}
