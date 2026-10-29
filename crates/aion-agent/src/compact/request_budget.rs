//! Provider-neutral request sizing and non-destructive historical image projection.
use aion_types::llm::LlmRequest;
use aion_types::message::{ContentBlock, Message};
use serde_json::{json, to_vec};

/// A conservative content budget, not a claim about a provider's exact wire limit.
/// Provider-specific envelopes add overhead; hosts may configure a lower limit.
pub(crate) fn payload_bytes(request: &LlmRequest) -> usize {
    to_vec(&json!({"model":request.model,"system":request.system,"messages":request.messages,"tools":request.tools.iter().map(|tool|json!({"name":tool.name,"description":tool.description,"input_schema":tool.input_schema})).collect::<Vec<_>>()}))
        .map(|bytes| bytes.len()).unwrap_or(usize::MAX)
}

/// Project request copies only. None means all images (text-only summarization).
pub(crate) fn omit_images(messages: &mut [Message], active_turn: Option<&str>) {
    for message in messages {
        if active_turn.is_some() && message.turn_id.as_deref() == active_turn {
            continue;
        }
        for block in &mut message.content {
            if matches!(block, ContentBlock::Image { .. }) {
                *block = ContentBlock::Text { text: "[Historical image omitted from this request. Its visual content is unavailable here; reopen the original attachment with ViewImage if needed. Do not infer its contents from this marker.]".into() };
            }
        }
    }
}

#[cfg(test)]
#[path = "request_budget_test.rs"]
mod tests;
