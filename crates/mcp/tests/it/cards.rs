//! Which results get a card, and the numbers that tell a lane's newest card
//! from its older ones.

use rmcp::model::{CallToolResult, ElicitationAction};
use serde_json::{Value, json};

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// The `view` and `card` of a result.
fn card(result: &CallToolResult) -> (Value, Value) {
    let structured = result
        .structured_content
        .clone()
        .expect("structured content");
    (structured["view"].clone(), structured["card"].clone())
}

#[tokio::test]
/// Each card about one lane gets that lane's next number, whichever command
/// made it; another lane counts from 1, and calls at the same time each get
/// their own number. `app_snapshot` returns the newest of the lanes it lists.
async fn cards_count_per_lane() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new().with_ui(), Lifecycle::Initialize, |_| {}).await;
        let status = harness.call_with("status", json!({"lane": "acme@1"})).await;
        assert!(!failed(&status), "{}", text(&status));
        assert_eq!(card(&status), (json!("lane"), json!(1)));
        let start = harness.call_with("start", json!({"lane": "acme@1"})).await;
        assert!(!failed(&start), "{}", text(&start));
        assert_eq!(card(&start), (json!("lane"), json!(2)));
        let other = harness.call_with("status", json!({"lane": "acme@2"})).await;
        assert_eq!(card(&other), (json!("lane"), json!(1)));

        let call = || harness.call("devlanes", &["status", "acme@1"]);
        let (a, b, c) = tokio::join!(call(), call(), call());
        let mut numbers: Vec<Value> = [a, b, c].iter().map(|result| card(result).1).collect();
        numbers.sort_by_key(|number| number.as_u64());
        assert_eq!(numbers, [json!(3), json!(4), json!(5)]);

        let snapshot = harness.call_with("app_snapshot", json!({})).await;
        let structured = snapshot.structured_content.expect("structured content");
        assert_eq!(structured["newest_card"], json!({"acme@1": 5}));
        let snapshot = harness
            .call_with("app_snapshot", json!({"lane": "acme@2"}))
            .await;
        let structured = snapshot.structured_content.expect("structured content");
        assert_eq!(structured["newest_card"], json!({"acme@2": 1}));
        // Snapshots number nothing.
        let again = harness.call_with("status", json!({"lane": "acme@2"})).await;
        assert_eq!(card(&again), (json!("lane"), json!(2)));
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// Cards about no lane or several have no number; a lane that never had a
/// card is not in `newest_card`.
async fn only_cards_about_one_lane_are_numbered() {
    tokio::time::timeout(LIMIT, async {
        // Accepts the question `daemon pair` asks.
        let client = TestClient::new()
            .with_ui()
            .answering(ElicitationAction::Accept);
        let harness = connect(client, Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_BOARD".into(), "1".into());
        })
        .await;
        let board = harness.call_with("status", json!({})).await;
        assert_eq!(card(&board), (json!("board"), Value::Null));
        let pair = harness.call_with("daemon_pair", json!({})).await;
        assert_eq!(card(&pair), (json!("pair"), Value::Null));
        // Log lines name their unit only.
        let logs = harness
            .call_with("logs", json!({"units": ["acme@1:api"]}))
            .await;
        assert_eq!(card(&logs), (json!("failure"), Value::Null));
        // `debug top` data is the `debug top --json` document as it is.
        let top = harness.call_with("debug_top", json!({})).await;
        assert_eq!(card(&top), (json!("top"), Value::Null));
        assert_eq!(
            top.structured_content.expect("structured content")["data"],
            json!({"lanes": []})
        );
        let start = harness.call_with("start", json!({"lane": "acme@2"})).await;
        assert_eq!(card(&start), (json!("lane"), json!(1)));
        let snapshot = harness.call_with("app_snapshot", json!({})).await;
        let structured = snapshot.structured_content.expect("structured content");
        assert_eq!(structured["newest_card"], json!({"acme@2": 1}));
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A result that names one lane but shows no card takes no number, so the
/// lane's card the user sees stays the newest.
async fn results_without_a_card_take_no_number() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            let units = json!({"items": [{"name": "api", "lane": "acme@1"}]});
            config.env.insert("FAKE_DATA".into(), units.to_string());
        })
        .await;
        let stop = harness
            .call_with("stop", json!({"units": ["acme@1:api"]}))
            .await;
        assert_eq!(card(&stop), (Value::Null, Value::Null));
        let errors = harness.call_with("debug_errors", json!({})).await;
        assert_eq!(card(&errors), (json!("failure"), json!(1)));
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// Only a plain `logs UNIT` gets a card: the logs an agent narrows down with a
/// query are already in the text.
async fn log_queries_get_no_card() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |_| {}).await;
        for arguments in [
            json!({"units": ["api"]}),
            json!({"units": ["api"], "tail": 20}),
        ] {
            let logs = harness.call_with("logs", arguments.clone()).await;
            assert_eq!(card(&logs).0, "failure", "{arguments}");
        }
        for arguments in [
            json!({"units": ["api"], "around": "api#120"}),
            json!({"units": ["api"], "grep": "error"}),
            json!({"units": ["api"], "cursor": "api#2"}),
            json!({"units": ["api"], "since": "5m"}),
            json!({"units": ["api"], "head": 10}),
            json!({"units": ["api"], "level": "warn"}),
            json!({"units": ["api"], "follow": true, "timeout": "1s"}),
        ] {
            let logs = harness.call_with("logs", arguments.clone()).await;
            assert!(!failed(&logs), "{arguments}: {}", text(&logs));
            assert_eq!(card(&logs), (Value::Null, Value::Null), "{arguments}");
        }
    })
    .await
    .expect("in time");
}
