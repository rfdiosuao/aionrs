use super::*;
use aion_types::message::{ContentBlock, ImageUrl, Role};

fn image(turn: &str) -> Message {
    let mut m = Message::new(
        Role::User,
        vec![ContentBlock::Image {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}", "AAAA".repeat(400_000)),
            },
        }],
    );
    m.turn_id = Some(turn.into());
    m
}
fn request(messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        model: "test".into(),
        system: String::new(),
        messages,
        tools: vec![],
        max_tokens: None,
        thinking: None,
        reasoning_effort: None,
    }
}
#[test]
fn request_budget_counts_base64_bytes_independently_from_tokens() {
    assert!(payload_bytes(&request(vec![image("old")])) > 1_600_000);
}
#[test]
fn historical_images_are_projected_without_changing_original_or_current_turn() {
    let original = vec![image("old"), image("active")];
    let mut projected = original.clone();
    omit_images(&mut projected, Some("active"));
    assert!(matches!(&original[0].content[0], ContentBlock::Image { .. }));
    assert!(matches!(&projected[0].content[0],ContentBlock::Text {text} if text.contains("Historical image")));
    assert!(matches!(&projected[1].content[0], ContentBlock::Image { .. }));
}
#[test]
fn summaries_omit_all_images_but_preserve_text_and_tool_pairing() {
    let mut messages = vec![image("active")];
    messages[0].content.push(ContentBlock::Text {
        text: "Attachment path: example.png".into(),
    });
    messages.push(Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "call".into(),
            name: "Read".into(),
            input: serde_json::json!({"path":"example.png"}),
            extra: None,
        }],
    ));
    messages.push(Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "call".into(),
            content: "original result".into(),
            is_error: false,
        }],
    ));
    omit_images(&mut messages, None);
    assert!(
        messages[0]
            .content
            .iter()
            .all(|b| !matches!(b, ContentBlock::Image { .. }))
    );
    assert!(
        messages[0]
            .content
            .iter()
            .any(|b| matches!(b,ContentBlock::Text{text} if text.contains("example.png")))
    );
}

#[test]
fn request_budget_includes_tool_schemas_and_escaped_text() {
    let mut r = request(vec![]);
    let before = payload_bytes(&r);
    r.system = "\"\n".repeat(10_000);
    assert!(payload_bytes(&r) >= before + 40_000);
    let text_bytes = payload_bytes(&r);
    r.tools.push(aion_types::tool::ToolDef {
        name: "Read".into(),
        description: "d".repeat(4000),
        input_schema: serde_json::json!({"type":"object"}),
        deferred: false,
    });
    assert!(payload_bytes(&r) > text_bytes + 4000);
}
