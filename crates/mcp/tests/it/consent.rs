//! Commands that need the user's consent ask through elicitation before they
//! run, and run with `--yes` only after a yes.

use rmcp::model::ElicitationAction;

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// The consent list of the plan.
const ASKING: &[&[&str]] = &[
    &["lane", "clean", "2"],
    &["lane", "gc"],
    &["daemon", "pair"],
    &["daemon", "stop"],
    &["daemon", "restart"],
    &["daemon", "reset-state"],
    &["daemon", "revoke", "laptop"],
    &["remote", "connect", "box"],
    &["remote", "disconnect", "box"],
    &["config", "set", "remote.require_pairing", "false"],
    &["config", "unset", "remote.require_pairing"],
];

/// The command lines the fake ran for `args`, without the dry runs.
fn ran(harness: &crate::Harness) -> Vec<String> {
    harness
        .lines()
        .into_iter()
        .filter(|line| !line.ends_with("--dry-run"))
        .collect()
}

#[tokio::test]
/// After a yes each command runs once, with `--yes`; `lane clean` shows what
/// its dry run lists in the question.
async fn accepted_commands_run_with_yes() {
    tokio::time::timeout(LIMIT, async {
        for args in ASKING {
            let harness = connect(
                TestClient::new().answering(ElicitationAction::Accept),
                Lifecycle::Initialize,
                |_| {},
            )
            .await;
            let result = harness.call("devlanes", args).await;
            assert!(!failed(&result), "{args:?}: {}", text(&result));
            let questions = harness.handler.questions();
            assert_eq!(questions.len(), 1, "{args:?}");
            assert!(questions[0].contains(&args.join(" ")), "{}", questions[0]);
            assert_eq!(
                ran(&harness),
                [format!("--output agent {} --yes", args.join(" "))]
            );
            if args[0] == "lane" && args[1] == "clean" {
                assert!(
                    questions[0].contains("would delete the data of lane 2"),
                    "{}",
                    questions[0]
                );
            }
        }
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// After a no, or without elicitation, nothing runs and the result says why.
async fn declined_or_unasked_commands_do_not_run() {
    tokio::time::timeout(LIMIT, async {
        for args in ASKING {
            let declining = connect(
                TestClient::new().answering(ElicitationAction::Decline),
                Lifecycle::Initialize,
                |_| {},
            )
            .await;
            let result = declining.call("devlanes", args).await;
            assert!(failed(&result));
            assert!(text(&result).contains("declined"), "{}", text(&result));
            assert!(
                ran(&declining).is_empty(),
                "{args:?}: {:?}",
                declining.lines()
            );
            let unasked = connect(TestClient::new(), Lifecycle::Initialize, |_| {}).await;
            let result = unasked.call("devlanes", args).await;
            assert!(failed(&result));
            assert!(
                text(&result).contains("ask the user, then rerun with --yes"),
                "{}",
                text(&result)
            );
            assert!(
                unasked.lines().is_empty(),
                "{args:?}: {:?}",
                unasked.lines()
            );
        }
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A question the user dismissed is reported as cancelled, not declined, and
/// nothing runs.
async fn cancelled_questions_run_nothing() {
    tokio::time::timeout(LIMIT, async {
        for lifecycle in [Lifecycle::Initialize, Lifecycle::Discover] {
            let harness = connect(
                TestClient::new().answering(ElicitationAction::Cancel),
                lifecycle,
                |_| {},
            )
            .await;
            let result = harness.call("devlanes", &["daemon", "stop"]).await;
            assert!(failed(&result));
            assert!(text(&result).contains("cancelled"), "{}", text(&result));
            assert!(!text(&result).contains("declined"), "{}", text(&result));
            assert!(harness.lines().is_empty(), "{:?}", harness.lines());
        }
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// `devlanes_read` never answers a command's own question with a yes.
async fn the_read_tool_does_not_confirm() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().answering(ElicitationAction::Accept),
            Lifecycle::Initialize,
            |config| {
                config.env.insert("FAKE_CONFIRM".into(), "1".into());
            },
        )
        .await;
        let result = harness.call("devlanes_read", &["status"]).await;
        assert!(failed(&result));
        assert!(harness.handler.questions().is_empty());
        assert_eq!(harness.lines(), ["--output agent status"]);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// An agent that passes `--yes` asked the user itself.
async fn yes_from_the_agent_runs_directly() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().answering(ElicitationAction::Decline),
            Lifecycle::Initialize,
            |_| {},
        )
        .await;
        let result = harness
            .call("devlanes", &["lane", "clean", "2", "--yes"])
            .await;
        assert!(!failed(&result), "{}", text(&result));
        assert!(harness.handler.questions().is_empty());
        assert_eq!(harness.lines(), ["--output agent lane clean 2 --yes"]);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A command that exits 2 asking for `--yes` (a task with `confirm:`) is asked
/// about and run again after a yes; without elicitation the result says to ask.
async fn confirmation_required_is_asked_and_rerun() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().answering(ElicitationAction::Accept),
            Lifecycle::Initialize,
            |config| {
                config.env.insert("FAKE_CONFIRM".into(), "1".into());
            },
        )
        .await;
        let result = harness.call("devlanes", &["run", "deploy"]).await;
        assert!(!failed(&result), "{}", text(&result));
        assert!(harness.handler.questions()[0].contains("deploy now?"));
        assert_eq!(
            harness.lines(),
            [
                "--output agent run deploy",
                "--output agent run deploy --yes"
            ]
        );
        let unasked = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_CONFIRM".into(), "1".into());
        })
        .await;
        let result = unasked.call("devlanes", &["run", "deploy"]).await;
        assert!(failed(&result));
        assert!(
            text(&result).contains("ask the user, then rerun with --yes"),
            "{}",
            text(&result)
        );
        assert_eq!(unasked.lines(), ["--output agent run deploy"]);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// On 2026-07-28 the question travels in an `InputRequiredResult`, and the
/// client's second call carries the answer.
async fn discover_clients_answer_by_calling_again() {
    tokio::time::timeout(LIMIT, async {
        let accepting = connect(
            TestClient::new().answering(ElicitationAction::Accept),
            Lifecycle::Discover,
            |_| {},
        )
        .await;
        let result = accepting.call("devlanes", &["daemon", "stop"]).await;
        assert!(!failed(&result), "{}", text(&result));
        assert_eq!(accepting.handler.questions().len(), 1);
        assert_eq!(accepting.lines(), ["--output agent daemon stop --yes"]);
        let declining = connect(
            TestClient::new().answering(ElicitationAction::Decline),
            Lifecycle::Discover,
            |_| {},
        )
        .await;
        let result = declining.call("devlanes", &["daemon", "stop"]).await;
        assert!(failed(&result));
        assert!(declining.lines().is_empty(), "{:?}", declining.lines());
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// A yes whose request state the server did not give for this command line
/// is no answer: the server asks the user again.
async fn a_yes_for_another_command_asks_again() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(
            TestClient::new().answering(ElicitationAction::Decline),
            Lifecycle::Discover,
            |_| {},
        )
        .await;
        let mut params = rmcp::model::CallToolRequestParams::new("devlanes").with_arguments(
            serde_json::from_value(serde_json::json!({"args": ["daemon", "stop"]})).expect("args"),
        );
        params.input_responses = Some(
            [(
                "consent".to_owned(),
                serde_json::json!({"action": "accept"}),
            )]
            .into_iter()
            .collect(),
        );
        params.request_state = Some("--yes 0000000000000000".into());
        let result = harness.client.call_tool(params).await.expect("call");
        assert!(failed(&result), "{}", text(&result));
        assert_eq!(harness.handler.questions().len(), 1);
        assert!(harness.lines().is_empty(), "{:?}", harness.lines());
    })
    .await
    .expect("in time");
}
