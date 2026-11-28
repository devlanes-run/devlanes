//! The `gui_*` tools, which a server offers only while GUI services are on:
//! a screenshot comes back as image content, only when its file is a PNG.

use base64::Engine as _;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::json;

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// The images of a result.
fn images(result: &CallToolResult) -> Vec<&rmcp::model::ImageContent> {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

/// A screenshot whose JSON names a PNG has that PNG as image content next
/// to its text; one that names another file gets a note instead, and a
/// click has no image.
#[tokio::test]
async fn a_screenshot_returns_the_image() {
    tokio::time::timeout(LIMIT, async {
        let png = b"\x89PNG\r\n\x1a\nnot really a picture".to_vec();
        let dir = tempfile::tempdir().expect("dir");
        let (shot, other) = (dir.path().join("shot.png"), dir.path().join("notes.txt"));
        std::fs::write(&shot, &png).expect("write");
        std::fs::write(&other, "secret").expect("write");
        for (path, attached) in [(&shot, true), (&other, false)] {
            let data =
                json!({"lane": "acme@1", "unit": "desktop", "path": path, "width": 1, "height": 1});
            let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
                config.gui = true;
                config.env.insert("FAKE_DATA".into(), data.to_string());
            })
            .await;
            let result = harness
                .call_with("gui_screenshot", json!({"unit": "desktop"}))
                .await;
            assert!(!failed(&result), "{}", text(&result));
            let found = images(&result);
            if attached {
                let [image] = found.as_slice() else {
                    panic!("one image: {:?}", result.content);
                };
                assert_eq!(image.mime_type, "image/png");
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .expect("base64");
                assert_eq!(decoded, png);
            } else {
                assert!(found.is_empty(), "{:?}", result.content);
                assert!(text(&result).contains("is not a PNG"), "{}", text(&result));
            }
            let clicked = harness
                .call_with("gui_click", json!({"unit": "desktop", "x": 3, "y": 4}))
                .await;
            assert!(images(&clicked).is_empty(), "a click has no image");
        }
    })
    .await
    .expect("in time");
}
