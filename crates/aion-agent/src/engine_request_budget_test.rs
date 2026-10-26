use super::*;
use crate::output::null_sink::NullSink;
use aion_config::config::CliArgs;
use aion_providers::ProviderError;
use aion_types::message::ImageUrl;
use tokio::sync::mpsc;

#[derive(Default)]
struct BudgetProvider {
    requests: Mutex<Vec<LlmRequest>>,
    reject_normal: bool,
    fail_summary: bool,
    error_status: Option<u16>,
}
#[async_trait::async_trait]
impl LlmProvider for BudgetProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            let first = requests.is_empty();
            requests.push(request.clone());
            first
        };
        let summary = request.system == crate::compact::prompt::COMPACT_SYSTEM_PROMPT;
        if !summary && (first || self.reject_normal) {
            return Err(ProviderError::Api {
                status: self.error_status.unwrap_or(413),
                message: "too large".into(),
            });
        }
        if summary && self.fail_summary {
            return Err(ProviderError::Connection("summary unavailable".into()));
        }
        let (tx, rx) = mpsc::channel(4);
        tx.send(LlmEvent::TextDelta(
            if summary {
                "<summary>Earlier work preserved</summary>"
            } else {
                "done"
            }
            .into(),
        ))
        .await
        .unwrap();
        tx.send(LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        })
        .await
        .unwrap();
        Ok(rx)
    }
}
fn engine(provider: Arc<BudgetProvider>, budget: usize) -> AgentEngine {
    let args = CliArgs {
        provider: Some("anthropic".into()),
        api_key: Some("test".into()),
        model: Some("test".into()),
        base_url: None,
        max_tokens: None,
        thinking: None,
        thinking_budget: None,
        max_turns: None,
        max_tool_call_malformed_turns: None,
        max_tool_call_failure_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: None,
    };
    let mut config = Config::resolve(&args).unwrap();
    config.session.enabled = false;
    config.compact.request_byte_budget = budget;
    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        Arc::new(NullSink),
        std::env::temp_dir(),
    );
    engine.messages = (0..20)
        .map(|i| {
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: format!("Earlier {i}: {}", "x".repeat(800)),
                }],
            )
        })
        .collect();
    engine
}
#[tokio::test]
async fn byte_pressure_compacts_below_token_threshold_and_preserves_current_request() {
    let provider = Arc::new(BudgetProvider::default());
    let mut engine = engine(provider.clone(), 16000);
    let result = engine.run("Keep THIS exact new instruction", "new").await.unwrap();
    assert_eq!(result.text, "done");
    assert_eq!(engine.context_state.compact_count, 1);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests[0].system, crate::compact::prompt::COMPACT_SYSTEM_PROMPT);
    assert!(requests.last().unwrap().messages.iter().any(|m| {
        m.content
            .iter()
            .any(|b| matches!(b,ContentBlock::Text{text} if text=="Keep THIS exact new instruction"))
    }));
}
#[tokio::test]
async fn provider_413_recovers_once_without_reappending_user_or_replaying_run() {
    let provider = Arc::new(BudgetProvider::default());
    let mut engine = engine(provider.clone(), 2 * 1024 * 1024);
    assert_eq!(engine.run("new exact task", "new").await.unwrap().text, "done");
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    assert_eq!(
        engine
            .messages
            .iter()
            .filter(|m| m
                .content
                .iter()
                .any(|b| matches!(b,ContentBlock::Text{text} if text=="new exact task")))
            .count(),
        1
    );
}
#[tokio::test]
async fn persistent_413_stops_after_one_recovery() {
    let provider = Arc::new(BudgetProvider {
        reject_normal: true,
        ..Default::default()
    });
    let mut engine = engine(provider.clone(), 2 * 1024 * 1024);
    assert!(engine.run("task", "new").await.is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}
#[test]
fn historical_image_projection_keeps_original_payload_and_active_image() {
    let provider = Arc::new(BudgetProvider::default());
    let mut engine = engine(provider, 2 * 1024 * 1024);
    engine.compat.image_input = Some(ImageInputCapability::Supported);
    engine.current_turn_id = Some("active".into());
    let mut old = Message::new(
        Role::User,
        vec![ContentBlock::Image {
            image_url: ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
            },
        }],
    );
    old.turn_id = Some("old".into());
    let mut current = old.clone();
    current.turn_id = Some("active".into());
    engine.messages = vec![old, current];
    let request = engine.build_request(TurnKind::Normal);
    assert!(matches!(engine.messages[0].content[0], ContentBlock::Image { .. }));
    assert!(matches!(request.messages[0].content[0], ContentBlock::Text { .. }));
    assert!(matches!(request.messages[1].content[0], ContentBlock::Image { .. }));
}

#[tokio::test]
async fn failed_summary_does_not_replace_history_or_repeat_the_normal_request() {
    let provider = Arc::new(BudgetProvider {
        fail_summary: true,
        ..Default::default()
    });
    let mut engine = engine(provider.clone(), 2 * 1024 * 1024);
    let original = engine.messages.len();
    assert!(engine.run("keep task", "new").await.is_err());
    assert_eq!(engine.messages.len(), original + 1);
    assert_eq!(engine.context_state.compact_count, 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn non_size_failures_do_not_trigger_compaction_or_auto_replay() {
    let provider = Arc::new(BudgetProvider {
        error_status: Some(429),
        ..Default::default()
    });
    let mut engine = engine(provider.clone(), 2 * 1024 * 1024);
    assert!(engine.run("task", "new").await.is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(engine.context_state.compact_count, 0);
}
#[tokio::test]
async fn disabled_automatic_compaction_does_not_spend_on_recovery() {
    let provider = Arc::new(BudgetProvider::default());
    let mut engine = engine(provider.clone(), 2 * 1024 * 1024);
    engine.compact_config.enabled = false;
    assert!(engine.run("task", "new").await.is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn an_oversized_active_image_is_not_silently_discarded_or_sent() {
    let provider = Arc::new(BudgetProvider::default());
    let mut engine = engine(provider.clone(), 512);
    engine.messages.clear();
    engine.compat.image_input = Some(ImageInputCapability::Supported);
    let image = ContentBlock::Image {
        image_url: ImageUrl {
            url: format!("data:image/png;base64,{}", "AAAA".repeat(200)),
        },
    };
    assert!(engine.run_with_blocks(vec![image], "image").await.is_err());
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(matches!(engine.messages[0].content[0], ContentBlock::Image { .. }));
}
