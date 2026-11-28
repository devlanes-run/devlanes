//! The `gui_*` tools, which a server offers only while GUI services are on:
//! listing them, a screenshot that comes back as image content, and an input
//! call.

use base64::Engine as _;
use rmcp::model::ContentBlock;
use serde_json::json;

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// With GUI services on the `gui_*` tools are listed, and under
/// `--read-only` only the ones that change nothing.
#[tokio::test]
async fn gui_tools_are_listed_while_gui_services_are_on() {
    tokio::time::timeout(LIMIT, async {
        for read_only in [false, true] {
            let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
                config.gui = true;
                config.read_only = read_only;
            })
            .await;
            let tools = harness.client.list_tools(None).await.expect("tools/list");
            let gui: Vec<String> = tools
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .filter(|name| name.starts_with("gui_"))
                .collect();
            let expected: &[&str] = if read_only {
                &["gui_ls", "gui_screenshot"]
            } else {
                &[
                    "gui_ls",
                    "gui_screenshot",
                    "gui_click",
                    "gui_type",
                    "gui_key",
                    "gui_scroll",
                    "gui_resize",
                ]
            };
            assert_eq!(gui, expected, "read_only: {read_only}");
        }
    })
    .await
    .expect("in time");
}

/// A screenshot's result has the PNG the command wrote as image content,
/// next to its text and JSON; a click runs its command line.
#[tokio::test]
async fn a_screenshot_returns_the_image() {
    tokio::time::timeout(LIMIT, async {
        let png = b"\x89PNG\r\n\x1a\nnot really a picture".to_vec();
        let file = tempfile::NamedTempFile::new().expect("file");
        std::fs::write(file.path(), &png).expect("write");
        let data = json!({"lane": "acme@1", "unit": "desktop", "path": file.path(), "width": 1, "height": 1});
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.gui = true;
            config.env.insert("FAKE_DATA".into(), data.to_string());
        })
        .await;
        let result = harness
            .call_with("gui_screenshot", json!({"unit": "desktop"}))
            .await;
        assert!(!failed(&result), "{}", text(&result));
        let images: Vec<_> = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Image(image) => Some(image),
                _ => None,
            })
            .collect();
        let [image] = images.as_slice() else {
            panic!("one image: {:?}", result.content);
        };
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .expect("base64"),
            png
        );
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["data"]["unit"], "desktop");

        let clicked = harness
            .call_with("gui_click", json!({"unit": "desktop", "x": 3, "y": 4}))
            .await;
        assert!(!failed(&clicked), "{}", text(&clicked));
        assert!(
            clicked
                .content
                .iter()
                .all(|block| !matches!(block, ContentBlock::Image(_))),
            "a click has no image"
        );
        assert_eq!(
            harness.lines(),
            [
                "--output agent gui screenshot desktop",
                "--output agent gui click desktop 3 4"
            ]
        );
    })
    .await
    .expect("in time");
}
