use std::{collections::HashSet, sync::Arc};

use axum::{body::Body, http::Response};
use serde_json::Value;

use super::{
    Bridge, MessagesRequest,
    content::{ToolResult, collect_tool_results},
    internal_notification,
};
const ASYNC_LAUNCH_PREFIX: &str = "Async agent launched successfully.";
const BACKGROUND_MARKER: &str = "The agent is working in the background.";
impl Bridge {
    pub(super) async fn async_agent_launch_handoff(
        &self,
        request: &MessagesRequest,
    ) -> Option<Response<Body>> {
        let round_tool_use_ids = latest_tool_round_ids(request)?;
        let message = request.messages.last()?;
        let tool_use_ids = exact_async_launch_acknowledgement(message, &round_tool_use_ids)?;
        let launches = self.agent_efforts.background_launches(&tool_use_ids)?;
        let results = collect_tool_results(std::slice::from_ref(request.messages.last()?));
        if !self.cancel_handed_off_provider_session(&results).await {
            return None;
        }
        tracing::info!(
            launch_count = launches.len(),
            "returned control after native Claude Code background Agent launch"
        );
        // Claude Code renders the launch/result in its native task panel from
        // the tool result. Do not add adapter-owned assistant narration: an
        // empty end_turn keeps the main prompt available without putting a
        // synthetic status line into the transcript or user-facing queue.
        Some(internal_notification::acknowledge(request))
    }
    async fn cancel_handed_off_provider_session(&self, results: &[ToolResult]) -> bool {
        let Some(session) = self.find_result_session(results).await else {
            return true;
        };
        let pending_ids = session
            .pending_tools
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if self
            .agent_efforts
            .background_launches(&pending_ids)
            .is_none()
        {
            return false;
        }
        let ready = self.app.ensure_thread_ready(&session.thread_id).await;
        if ready.is_err() {
            return false;
        }
        let events = Arc::new(self.app.subscribe_thread(&session.thread_id));
        // Do not abort the shared Codex provider; reject and drain this turn.
        self.disconnect_stream_for_async_handoff(&session, events)
            .await;
        true
    }
}
fn pure_async_launch_tool_results(message: &Value) -> Option<Vec<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let blocks = message.get("content")?.as_array()?;
    if blocks.is_empty() {
        return None;
    }
    blocks
        .iter()
        .map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_result")
                || block.get("is_error").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            let text = strict_result_text(block.get("content")?)?;
            if !text.trim_start().starts_with(ASYNC_LAUNCH_PREFIX)
                || !text.contains(BACKGROUND_MARKER)
            {
                return None;
            }
            block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// Return the successful async launch IDs only when they are a duplicate-free,
/// exact match for the expected tool round. Both handoff and the
/// scheduler use this predicate so a partial or replayed acknowledgement cannot
/// make the two lifecycle views diverge.
pub(crate) fn exact_async_launch_acknowledgement(
    message: &Value,
    expected_tool_use_ids: &[String],
) -> Option<Vec<String>> {
    let result_ids = pure_async_launch_tool_results(message)?;
    if result_ids.len() != expected_tool_use_ids.len() || result_ids.is_empty() {
        return None;
    }
    let result_set = result_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let expected_set = expected_tool_use_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if result_set.len() != result_ids.len()
        || expected_set.len() != expected_tool_use_ids.len()
        || result_set != expected_set
    {
        return None;
    }
    Some(result_ids)
}

fn strict_result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) if !items.is_empty() => {
            let mut text = String::new();
            for item in items {
                append_strict_result_text(&mut text, item)?;
            }
            Some(text)
        }
        _ => None,
    }
}

fn append_strict_result_text(text: &mut String, item: &Value) -> Option<()> {
    (item.get("type").and_then(Value::as_str) == Some("text")).then_some(())?;
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(item.get("text").and_then(Value::as_str)?);
    Some(())
}

fn latest_tool_round_ids(request: &MessagesRequest) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .skip(1)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .find_map(tool_round_ids)
}

pub(crate) fn tool_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(content: Value) -> MessagesRequest {
        MessagesRequest {
            model: "main-model".to_owned(),
            system: Value::Null,
            messages: vec![json!({"role":"user", "content":content})],
            tools: Vec::new(),
            stream: false,
            output_config: json!({}),
            metadata: json!({}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    fn launch_result(id: &str) -> Value {
        json!({
            "type":"tool_result",
            "tool_use_id":id,
            "content":[{"type":"text", "text":format!(
                "{ASYNC_LAUNCH_PREFIX}\nagentId: internal\n{BACKGROUND_MARKER}"
            )}]
        })
    }

    #[test]
    fn accepts_only_pure_successful_async_launch_results() {
        let pure = request(json!([launch_result("one"), launch_result("two")]));
        assert_eq!(
            pure_async_launch_tool_results(pure.messages.last().unwrap()),
            Some(vec!["one".to_owned(), "two".to_owned()])
        );

        let mixed_text = request(json!([launch_result("one"), {"type":"text", "text":"hi"}]));
        assert!(pure_async_launch_tool_results(mixed_text.messages.last().unwrap()).is_none());
        let failed = request(json!([{
            "type":"tool_result", "tool_use_id":"one", "is_error":true,
            "content":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")
        }]));
        assert!(pure_async_launch_tool_results(failed.messages.last().unwrap()).is_none());
        let completed = request(json!([{
            "type":"tool_result", "tool_use_id":"one", "content":"finished"
        }]));
        assert!(pure_async_launch_tool_results(completed.messages.last().unwrap()).is_none());
        let rich = request(json!([{
            "type":"tool_result", "tool_use_id":"one",
            "content":[{"type":"image"}, {"type":"text", "text":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")}]
        }]));
        assert!(pure_async_launch_tool_results(rich.messages.last().unwrap()).is_none());
    }

    #[test]
    fn requires_results_to_belong_to_the_latest_native_tool_round() {
        let mut correlated = request(json!([launch_result("background")]));
        correlated.messages.insert(
            0,
            json!({
                "role":"assistant",
                "content":[
                    {"type":"text", "text":"Launching delegated work."},
                    {"type":"tool_use", "id":"background", "name":"Agent", "input":{}},
                    {"type":"tool_use", "id":"other", "name":"Read", "input":{}}
                ]
            }),
        );
        assert_eq!(
            latest_tool_round_ids(&correlated),
            Some(vec!["background".to_owned(), "other".to_owned()])
        );

        let uncorrelated = request(json!([launch_result("background")]));
        assert!(latest_tool_round_ids(&uncorrelated).is_none());
    }

    #[test]
    fn requires_an_exact_unique_async_result_set() {
        let expected = vec!["one".to_owned(), "two".to_owned()];
        let exact = request(json!([launch_result("two"), launch_result("one")]));
        assert_eq!(
            exact_async_launch_acknowledgement(exact.messages.last().unwrap(), &expected),
            Some(vec!["two".to_owned(), "one".to_owned()])
        );

        let partial = request(json!([launch_result("one")]));
        assert!(
            exact_async_launch_acknowledgement(partial.messages.last().unwrap(), &expected)
                .is_none()
        );
        let duplicate = request(json!([launch_result("one"), launch_result("one")]));
        assert!(
            exact_async_launch_acknowledgement(duplicate.messages.last().unwrap(), &expected)
                .is_none()
        );
    }

    #[tokio::test]
    async fn background_handoff_returns_empty_native_end_turn_without_synthetic_text() {
        let json_request = request(json!([launch_result("one")]));
        let response = internal_notification::acknowledge(&json_request);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["stop_reason"], "end_turn");
        assert_eq!(body["content"], json!([]));
        assert!(!body.to_string().contains("Claude Code started"));

        let mut stream_request = json_request;
        stream_request.stream = true;
        let response = internal_notification::acknowledge(&stream_request);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: message_start"));
        assert!(!body.contains("content_block_delta"));
        assert!(body.contains(r#""stop_reason":"end_turn""#));
        assert!(body.contains("event: message_stop"));
    }
}

#[cfg(test)]
#[path = "async_agent_handoff_extra_tests.rs"]
mod extra_tests;
