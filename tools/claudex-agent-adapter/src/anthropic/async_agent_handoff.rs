use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

use axum::{
    body::Body,
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};

use super::{
    Bridge, MessagesRequest, Segment, Usage, WebEvidenceSummary,
    agent_effort::BackgroundLaunchIntent,
    content::{ToolResult, anthropic_response, collect_tool_results, estimated_tokens, sse},
    stream::message_start,
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
        let text = launch_status(&launches);
        tracing::info!(
            launch_count = launches.len(),
            "returned control after native Claude Code background Agent launch"
        );
        Some(handoff_response(
            request,
            &text,
            &self.request_model(request),
        ))
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
        let events = Arc::new(self.app.subscribe_thread(&session.thread_id));
        self.disconnect_stream(&session, events).await;
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
/// exact match for the expected tool round. Both the Anthropic handoff and the
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

fn launch_status(launches: &[BackgroundLaunchIntent]) -> String {
    let models = launches
        .iter()
        .filter_map(|launch| launch.model.as_deref())
        .collect::<BTreeSet<_>>();
    let model_status = if models.is_empty() {
        String::new()
    } else {
        format!(
            " Models: {}.",
            models.into_iter().collect::<Vec<_>>().join(", ")
        )
    };
    format!(
        "Claude Code started {} native background SubAgent(s).{} They remain active and visible in Claude Code while the main prompt accepts the next instruction.",
        launches.len(),
        model_status
    )
}

fn handoff_response(request: &MessagesRequest, text: &str, model: &str) -> Response<Body> {
    let input_tokens = u64::try_from(super::token_count(request)).unwrap_or(u64::MAX);
    let output_tokens = estimated_tokens(text);
    if !request.stream {
        return anthropic_response(
            Segment {
                blocks: vec![json!({"type":"text", "text":text})],
                stop_reason: "end_turn",
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    web_search_requests: 0,
                },
                web_evidence: WebEvidenceSummary::default(),
            },
            model,
        );
    }
    let body = [
        message_start(model, input_tokens),
        sse(
            "content_block_start",
            json!({
                "type":"content_block_start", "index":0,
                "content_block":{"type":"text", "text":""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta", "index":0,
                "delta":{"type":"text_delta", "text":text}
            }),
        ),
        sse(
            "content_block_stop",
            json!({"type":"content_block_stop", "index":0}),
        ),
        sse(
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn", "stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
            }),
        ),
        sse("message_stop", json!({"type":"message_stop"})),
    ]
    .concat();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from(body))
        .expect("valid async Agent handoff response")
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
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
    async fn emits_valid_dynamic_json_and_streaming_end_turn_responses() {
        let launches = [
            BackgroundLaunchIntent {
                model: Some("worker-a".to_owned()),
            },
            BackgroundLaunchIntent {
                model: Some("worker-b".to_owned()),
            },
        ];
        let text = launch_status(&launches);
        assert!(text.contains("2 native background SubAgent(s)"));
        assert!(text.contains("worker-a, worker-b"));

        let json_request = request(json!([launch_result("one")]));
        let response = handoff_response(&json_request, &text, "main-model");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["stop_reason"], "end_turn");
        assert_eq!(body["content"][0]["text"], text);

        let mut stream_request = json_request;
        stream_request.stream = true;
        let response = handoff_response(&stream_request, &text, "main-model");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: message_start"));
        assert!(body.contains("event: content_block_delta"));
        assert!(body.contains(r#""stop_reason":"end_turn""#));
        assert!(body.contains("event: message_stop"));
    }
}
