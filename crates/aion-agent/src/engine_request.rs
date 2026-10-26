//! Per-request byte preflight and one bounded recovery before any stream is consumed.
use super::{AgentEngine, AgentError, TurnKind};
use crate::compact::request_budget::payload_bytes;
use aion_providers::ProviderError;
use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{ContentBlock, Message, Role};
use tokio::sync::mpsc::Receiver;
use tracing::info;

impl AgentEngine {
    pub(super) fn compaction_input(&self) -> Result<(Vec<Message>, Vec<Message>), AgentError> {
        let Some(id) = &self.current_turn_id else {
            return Ok((self.messages.clone(), vec![]));
        };
        let start = self
            .messages
            .iter()
            .position(|m| m.turn_id.as_ref() == Some(id))
            .unwrap_or(self.messages.len());
        let active = &self.messages[start..];
        let bytes = serde_json::to_vec(active).map(|v| v.len()).unwrap_or(usize::MAX);
        let long_active =
            self.compact_config.request_byte_budget > 0 && bytes > self.compact_config.request_byte_budget / 2;
        if start > 0 && (!long_active || active.len() <= 1) {
            return Ok((self.messages[..start].to_vec(), active.to_vec()));
        }
        if active.len() <= 1 {
            return Err(ProviderError::PromptTooLong(
                "The current turn alone exceeds the request budget; reduce its attachment or tool output".into(),
            )
            .into());
        }
        // A long tool loop must itself be compactable. Retain the exact user input,
        // the most recent complete tool group and every active image, without
        // dangling old ToolResult blocks or executing any tool again.
        let tail = active
            .iter()
            .rposition(|m| {
                m.role == Role::Assistant && m.content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            })
            .unwrap_or(active.len());
        let mut retained = vec![active[0].clone()];
        for message in &active[1..tail] {
            let images: Vec<_> = message
                .content
                .iter()
                .filter(|b| matches!(b, ContentBlock::Image { .. }))
                .cloned()
                .collect();
            if !images.is_empty() {
                let mut m = message.clone();
                m.role = Role::User;
                m.content = images;
                retained.push(m);
            }
        }
        if tail > 0 {
            retained.extend_from_slice(&active[tail..]);
        }
        Ok((self.messages.clone(), retained))
    }
    fn check_request_budget(&self, request: &LlmRequest) -> Result<(), AgentError> {
        let limit = self.compact_config.request_byte_budget;
        let bytes = payload_bytes(request);
        if limit > 0 && bytes > limit {
            return Err(ProviderError::PromptTooLong(format!("Request content exceeds the configured byte budget ({bytes} > {limit}); compact context or reduce the current attachment/tool payload")).into());
        }
        Ok(())
    }
    pub(super) async fn stream_with_request_recovery(
        &mut self,
        kind: TurnKind,
    ) -> Result<Receiver<LlmEvent>, AgentError> {
        let request = self.build_request(kind);
        self.check_request_budget(&request)?;
        match self.provider.stream(&request).await {
            Err(error)
                if self.compact_config.enabled
                    && matches!(
                        error,
                        ProviderError::Api { status: 413, .. } | ProviderError::PromptTooLong(_)
                    ) =>
            {
                // No stream was accepted: retry only this request, never engine.run or tools.
                info!(target: "aion_agent", "recovering rejected oversized request by compacting history once");
                self.run_compaction_with_force(true).await?;
                let request = self.build_request(kind);
                self.check_request_budget(&request)?;
                Ok(self.provider.stream(&request).await?)
            }
            result => Ok(result?),
        }
    }
}
