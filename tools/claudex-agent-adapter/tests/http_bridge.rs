mod support;

use std::time::{Duration, Instant};

use reqwest::{Client, Response};
use serde_json::{Value, json};
use support::{Adapter, base_request, post_json};

fn messages_url(adapter: &Adapter) -> String {
    format!("{}/v1/messages", adapter.base_url)
}

fn lookup_tools() -> Value {
    json!([{
        "name":"lookup",
        "description":"Look up a value",
        "input_schema":{
            "type":"object",
            "properties":{"key":{"type":"string"}},
            "required":["key"]
        }
    }])
}

async fn wait_for_session_slot_release(client: &Client, adapter: &Adapter) {
    loop {
        let health: Value = client
            .get(format!("{}/health", adapter.base_url))
            .send()
            .await
            .expect("read adapter health while draining")
            .json()
            .await
            .expect("decode adapter health while draining");
        if health["session_slots_used"] == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn active_request_counts(client: &Client, adapter: &Adapter) -> (u64, u64) {
    let health: Value = client
        .get(format!("{}/health", adapter.base_url))
        .send()
        .await
        .expect("read adapter health counters")
        .json()
        .await
        .expect("decode adapter health counters");
    (
        health["active_http_requests"]
            .as_u64()
            .expect("active HTTP request count"),
        health["active_provider_turns"]
            .as_u64()
            .expect("active provider turn count"),
    )
}

async fn count_tokens_for_session(
    client: &Client,
    adapter: &Adapter,
    session_id: &str,
    request: &Value,
) -> u64 {
    client
        .post(format!("{}/v1/messages/count_tokens", adapter.base_url))
        .header("x-claude-code-session-id", session_id)
        .json(request)
        .send()
        .await
        .expect("count session tokens")
        .json::<Value>()
        .await
        .expect("decode session token count")["input_tokens"]
        .as_u64()
        .expect("numeric input token count")
}

async fn wait_for_active_request_counts(client: &Client, adapter: &Adapter, expected: (u64, u64)) {
    tokio::time::timeout(
        Duration::from_secs(2),
        poll_active_request_counts(client, adapter, expected),
    )
    .await
    .expect("active request counters did not reach the expected values");
}

async fn poll_active_request_counts(client: &Client, adapter: &Adapter, expected: (u64, u64)) {
    while active_request_counts(client, adapter).await != expected {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn read_until_contains(
    response: &mut Response,
    stream: &mut String,
    expected: &str,
    early_end_message: &str,
) {
    while !stream.contains(expected) {
        let chunk = response
            .chunk()
            .await
            .expect("read response stream")
            .expect(early_end_message);
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
}

async fn finish_counted_response(mut response: Response, drop_early: bool) {
    if drop_early {
        return;
    }
    while response
        .chunk()
        .await
        .expect("read counted stream remainder")
        .is_some()
    {}
}

fn assert_subscription_stream_completion(response: &str, stream: bool) {
    if stream {
        assert!(response.contains("event: message_stop"));
    }
}

#[tokio::test]
async fn authenticates_protected_routes_but_keeps_health_public() {
    let adapter = Adapter::start_authenticated("test-secret").await;
    let client = Client::new();
    let health = client
        .get(format!("{}/health", adapter.base_url))
        .send()
        .await
        .expect("request public health");
    assert!(health.status().is_success());
    let health: Value = health.json().await.expect("decode health response");
    assert_eq!(health["session_capacity"], 1_024);
    assert_eq!(health["session_slots_used"], 0);
    assert_eq!(health["subscription_max_processes"], 20);
    assert_eq!(health["subscription_timeout_minutes"], 120);

    let models_url = format!("{}/v1/models", adapter.base_url);
    let unauthorized = client
        .get(&models_url)
        .send()
        .await
        .expect("request without adapter token");
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    for request in [
        client.get(&models_url).bearer_auth("test-secret"),
        client.get(&models_url).header("x-api-key", "test-secret"),
        client
            .get(&models_url)
            .header("x-api-key", "wrong")
            .bearer_auth("test-secret"),
    ] {
        assert!(
            request
                .send()
                .await
                .expect("request with token")
                .status()
                .is_success()
        );
    }
    let rejected = client
        .get(&models_url)
        .header("x-api-key", "wrong")
        .bearer_auth("wrong")
        .send()
        .await
        .expect("request with wrong tokens");
    assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rejects_an_invalid_disabled_subagent_model_header() {
    let adapter = Adapter::start().await;
    let response = Client::new()
        .post(messages_url(&adapter))
        .header("x-claudex-disabled-subagent-models", "model with spaces")
        .json(&base_request())
        .send()
        .await
        .expect("send invalid policy header");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn serves_models_counts_plain_messages_and_continuations() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let models: Value = client
        .get(format!("{}/v1/models", adapter.base_url))
        .send()
        .await
        .expect("request models")
        .json()
        .await
        .expect("decode models");
    assert_eq!(models["data"][0]["id"], "claude-claudex-test-main-model");
    assert_eq!(models["data"][0]["display_name"], "test-main-model");
    assert!(
        models["data"]
            .as_array()
            .expect("model list")
            .iter()
            .filter_map(|model| model["id"].as_str())
            .any(|id| id == "claude-claudex-test-main-model")
    );
    assert!(
        models["data"]
            .as_array()
            .expect("model list")
            .iter()
            .all(|model| {
                model["type"] == "model"
                    && model["display_name"]
                        .as_str()
                        .is_some_and(|name| !name.is_empty())
            })
    );

    let count = post_json(
        &client,
        &format!("{}/v1/messages/count_tokens", adapter.base_url),
        base_request(),
    )
    .await;
    assert!(count["input_tokens"].as_u64().unwrap_or_default() > 0);

    let plain = post_json(&client, &messages_url(&adapter), base_request()).await;
    assert_eq!(plain["content"][0]["text"], "OK");
    assert_eq!(plain["stop_reason"], "end_turn");
    assert_eq!(plain["usage"]["input_tokens"], 17);
    assert_eq!(plain["usage"]["output_tokens"], 3);

    let mut discovered = base_request();
    discovered["model"] = json!("claude-claudex-test-main-model");
    let discovery_adapter = Adapter::start().await;
    let discovered = post_json(&client, &messages_url(&discovery_adapter), discovered).await;
    assert_eq!(discovered["content"][0]["text"], "OK");

    let continued = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Test system prompt",
            "output_config":{"effort":"low"},
            "messages":[
                {"role":"user","content":"Say OK"},
                {"role":"assistant","content":plain["content"]},
                {"role":"user","content":"Say OK again"}
            ]
        }),
    )
    .await;
    assert_eq!(continued["content"][0]["text"], "OK");
    assert_eq!(continued["model"], "test-main-model");

    let missing_response = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"",
            "messages":[{"role":"user","content":"Say OK"}]
        }))
        .send()
        .await
        .expect("send missing-model request");
    assert_eq!(missing_response.status(), reqwest::StatusCode::BAD_REQUEST);
    let missing_model: Value = missing_response
        .json()
        .await
        .expect("decode missing-model error");
    assert_eq!(missing_model["type"], "error");
    assert!(
        missing_model["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("request model is required"))
    );

    let mut edge = base_request();
    edge["system"] = json!("");
    edge["tools"] = json!([
        {"description":"missing name"},
        {"name":"odd tool/name","description":"sanitized tool"}
    ]);
    let edge = post_json(&client, &messages_url(&adapter), edge).await;
    assert_eq!(edge["content"][0]["text"], "OK");
}

#[tokio::test]
async fn count_tokens_restores_only_the_matching_transport_identity() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut small = base_request();
    small["tools"] = lookup_tools();
    let mut large = base_request();
    large["tools"] = json!([{
        "name":"large_lookup",
        "description":"x".repeat(2_048),
        "input_schema":{"type":"object","properties":{}}
    }]);

    let small_count = count_tokens_for_session(&client, &adapter, "session-small", &small).await;
    let large_count = count_tokens_for_session(&client, &adapter, "session-large", &large).await;
    assert_ne!(small_count, large_count);

    let mut omitted = base_request();
    omitted
        .as_object_mut()
        .expect("request object")
        .remove("tools");
    assert_eq!(
        count_tokens_for_session(&client, &adapter, "session-small", &omitted).await,
        small_count
    );
    assert_eq!(
        count_tokens_for_session(&client, &adapter, "session-large", &omitted).await,
        large_count
    );

    let mut explicit_empty = omitted.clone();
    explicit_empty["tools"] = json!([]);
    let empty_count =
        count_tokens_for_session(&client, &adapter, "session-empty", &explicit_empty).await;
    assert_eq!(
        count_tokens_for_session(&client, &adapter, "session-small", &explicit_empty).await,
        empty_count
    );
}

#[tokio::test]
async fn ignores_oversized_provider_events_that_the_bridge_does_not_consume() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut request = base_request();
    request["messages"] = json!([{"role":"user","content":"OVERSIZED_IGNORED_EVENT"}]);

    let response = post_json(&client, &messages_url(&adapter), request).await;

    assert_eq!(response["content"][0]["text"], "OK");
    assert_eq!(response["stop_reason"], "end_turn");
}

#[tokio::test]
async fn renders_codex_provider_tools_as_progress_without_executable_tool_use() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut request = base_request();
    request["messages"] = json!([{"role":"user","content":"PROVIDER_TOOL_PROGRESS"}]);

    let response = post_json(&client, &messages_url(&adapter), request).await;

    assert_eq!(response["stop_reason"], "end_turn");
    let content = response["content"].as_array().expect("response content");
    assert!(content.iter().all(|block| block["type"] != "tool_use"));
    let text = content
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    assert!(!text.contains("▶ Read config"), "response={response}");
    assert!(
        text.contains("CODEX_PROVIDER_PROGRESS_OK"),
        "response={response}"
    );
}

#[tokio::test]
async fn bounds_reconstructed_history_below_the_app_server_input_limit() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut request = base_request();
    request["messages"] = json!([
        {"role":"user","content":format!("old-{}", "x".repeat(550_000))},
        {"role":"assistant","content":format!("middle-{}", "y".repeat(550_000))},
        {"role":"user","content":"LATEST_LIMIT_CHECK"}
    ]);

    let response = post_json(&client, &messages_url(&adapter), request).await;

    assert_eq!(response["content"][0]["text"], "OK");
    assert_eq!(response["stop_reason"], "end_turn");
}

#[tokio::test]
async fn streams_text_before_the_turn_completes() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut request = base_request();
    request["stream"] = json!(true);
    request["system"] = json!("Streaming test");
    request["messages"] = json!([{"role":"user","content":"STREAMING_DELAY"}]);
    let started = Instant::now();
    let mut response = client
        .post(messages_url(&adapter))
        .json(&request)
        .send()
        .await
        .expect("request stream");
    let mut stream = String::new();
    read_until_contains(
        &mut response,
        &mut stream,
        "event: message_start",
        "stream ended before message_start",
    )
    .await;
    let message_start_at = started.elapsed();
    assert!(
        message_start_at < Duration::from_millis(150),
        "message_start was buffered behind provider setup: {message_start_at:?}"
    );
    read_until_contains(
        &mut response,
        &mut stream,
        "FIRST",
        "stream ended before first delta",
    )
    .await;
    let first_text_at = started.elapsed();
    while let Some(chunk) = response.chunk().await.expect("read stream remainder") {
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    let completion_at = started.elapsed();
    assert!(
        completion_at.saturating_sub(first_text_at) >= Duration::from_millis(100),
        "first text delta was buffered until completion: first={first_text_at:?}, completion={completion_at:?}"
    );
    for expected in [
        "event: message_start",
        "event: content_block_delta",
        "SECOND",
        "event: message_stop",
    ] {
        assert!(
            stream.contains(expected),
            "missing SSE fragment: {expected}"
        );
    }
}

#[tokio::test]
async fn tracks_streaming_requests_until_body_eof_or_drop() {
    let adapter = Adapter::start().await;
    let client = Client::new();

    for drop_early in [false, true] {
        let mut request = base_request();
        request["stream"] = json!(true);
        request["messages"] = json!([{
            "role":"user",
            "content":format!(
                "STREAMING_DELAY COUNTER_{}",
                if drop_early { "DROP" } else { "EOF" }
            )
        }]);
        let mut response = client
            .post(messages_url(&adapter))
            .json(&request)
            .send()
            .await
            .expect("request counted stream");
        let mut stream = String::new();
        read_until_contains(
            &mut response,
            &mut stream,
            "FIRST",
            "counted stream ended before first delta",
        )
        .await;

        assert_eq!(active_request_counts(&client, &adapter).await, (1, 1));
        finish_counted_response(response, drop_early).await;
        wait_for_active_request_counts(&client, &adapter, (0, 0)).await;
    }
}

#[tokio::test]
async fn drains_a_non_cancellable_codex_turn_after_stream_disconnect() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut response = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "stream":true, "system":"Disconnect drain test",
            "tools":lookup_tools(),
            "messages":[{"role":"user","content":adapter.codex_disconnect_prompt()}]
        }))
        .send()
        .await
        .expect("start Codex stream");
    let mut stream = String::new();
    while !stream.contains("DISCONNECT_READY") {
        let chunk = response
            .chunk()
            .await
            .expect("read disconnect stream")
            .expect("stream ended before disconnect marker");
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    drop(response);
    adapter.wait_for_codex_disconnect_drain().await;

    let report = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Disconnect drain report",
            "messages":[{"role":"user","content":"REPORT_DISCONNECT_DRAIN"}]
        }),
    )
    .await;
    assert_eq!(report["content"][0]["text"], "CODEX_DISCONNECT_DRAINED");
}

#[tokio::test]
async fn releases_the_session_slot_while_a_disconnected_codex_turn_drains() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let mut response = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "stream":true, "system":"Slow disconnect drain test",
            "tools":lookup_tools(),
            "messages":[{"role":"user","content":adapter.codex_slow_disconnect_prompt()}]
        }))
        .send()
        .await
        .expect("start slow Codex stream");
    let mut stream = String::new();
    while !stream.contains("DISCONNECT_READY") {
        let chunk = response
            .chunk()
            .await
            .expect("read slow disconnect stream")
            .expect("stream ended before disconnect marker");
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    drop(response);

    tokio::time::timeout(
        Duration::from_millis(300),
        wait_for_session_slot_release(&client, &adapter),
    )
    .await
    .expect("disconnect must release the session slot before the slow tool event");

    adapter.wait_for_codex_disconnect_drain().await;
    let report = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Slow disconnect drain report",
            "messages":[{"role":"user","content":"REPORT_DISCONNECT_DRAIN"}]
        }),
    )
    .await;
    assert_eq!(report["content"][0]["text"], "CODEX_DISCONNECT_DRAINED");
}

#[tokio::test]
async fn completes_an_external_tool_round_trip_after_a_signature_change() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let tools = lookup_tools();
    let first = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "max_tokens":256, "system":"Tool test",
            "tools":tools, "messages":[{"role":"user","content":"USE_TOOL"}]
        }),
    )
    .await;
    assert_eq!(first["stop_reason"], "tool_use", "response={first}");
    assert_eq!(first["content"][0]["name"], "lookup");

    let second = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "max_tokens":256,
            "system":"Tool test with a changed request signature", "tools":tools,
            "messages":[
                {"role":"user","content":"USE_TOOL"},
                {"role":"assistant","content":first["content"]},
                {"role":"user","content":[{
                    "type":"tool_result", "tool_use_id":first["content"][0]["id"],
                    "content":"VALUE-42"
                }]}
            ]
        }),
    )
    .await;
    assert_eq!(second["content"][0]["text"], "VALUE-42");
    assert_eq!(second["stop_reason"], "end_turn");
}

#[tokio::test]
async fn recovers_a_tool_result_after_adapter_session_loss() {
    let first_adapter = Adapter::start().await;
    let client = Client::new();
    let tools = lookup_tools();
    let first = post_json(
        &client,
        &messages_url(&first_adapter),
        json!({
            "model":"test-main-model", "system":"Recovery test", "tools":tools,
            "messages":[{"role":"user","content":"USE_TOOL RECOVER_ORPHAN_TOOL_RESULT"}]
        }),
    )
    .await;
    assert_eq!(first["stop_reason"], "tool_use", "response={first}");
    drop(first_adapter);

    let restarted_adapter = Adapter::start().await;
    let recovered = post_json(
        &client,
        &messages_url(&restarted_adapter),
        json!({
            "model":"test-main-model", "system":"Recovery test", "tools":tools,
            "messages":[
                {"role":"user","content":"USE_TOOL RECOVER_ORPHAN_TOOL_RESULT"},
                {"role":"assistant","content":first["content"]},
                {"role":"user","content":[{
                    "type":"tool_result", "tool_use_id":first["content"][0]["id"],
                    "content":"VALUE-42"
                }]}
            ]
        }),
    )
    .await;
    assert_eq!(
        recovered["content"][0]["text"],
        "RECOVERED_ORPHAN_TOOL_RESULT"
    );
    assert_eq!(recovered["stop_reason"], "end_turn");
}

#[tokio::test]
async fn returns_parallel_and_streamed_tool_calls() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let tools = lookup_tools();
    let parallel = tokio::time::timeout(
        Duration::from_secs(2),
        post_json(
            &client,
            &messages_url(&adapter),
            json!({
                "model":"test-main-model", "system":"Parallel tool test", "tools":tools,
                "messages":[{"role":"user","content":"USE_PARALLEL_TOOLS"}]
            }),
        ),
    )
    .await
    .expect("external tool batch deadlocked");
    assert_eq!(parallel["stop_reason"], "tool_use", "response={parallel}");
    assert_eq!(parallel["content"].as_array().unwrap().len(), 2);

    let streamed = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "stream":true, "system":"Streaming tool test",
            "tools":tools, "messages":[{"role":"user","content":"TEXT_THEN_TOOL"}]
        }))
        .send()
        .await
        .expect("request streaming tool")
        .text()
        .await
        .expect("read streaming tool response");
    for expected in [
        "BEFORE_TOOL",
        "input_json_delta",
        "\"index\":0",
        "\"index\":1",
    ] {
        assert!(
            streamed.contains(expected),
            "missing tool stream fragment: {expected}"
        );
    }
}

#[tokio::test]
async fn ignores_per_item_completion_while_collecting_external_tools() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        post_json(
            &client,
            &messages_url(&adapter),
            json!({
                "model":"test-main-model", "system":"Interleaved tool events",
                "tools":lookup_tools(),
                "messages":[{"role":"user","content":"USE_INTERLEAVED_TOOLS"}]
            }),
        ),
    )
    .await
    .expect("per-item completion terminated or deadlocked the batch");
    assert_eq!(response["stop_reason"], "tool_use", "response={response}");
    assert_eq!(response["content"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn handles_retry_failed_turn_and_detached_errors() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let retry = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Retry test",
            "messages":[{"role":"user","content":"RETRY_THEN_OK"}]
        }),
    )
    .await;
    assert_eq!(retry["content"][0]["text"], "OK_AFTER_RETRY");

    let failed = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "system":"Failed turn test",
            "messages":[{"role":"user","content":"TURN_FAILED"}]
        }))
        .send()
        .await
        .expect("request failed turn");
    assert_eq!(failed.status(), reqwest::StatusCode::BAD_GATEWAY);

    let detached = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "stream":true, "system":"Detached error test",
            "messages":[{"role":"user","content":"DETACHED_ERROR"}]
        }))
        .send()
        .await
        .expect("request detached failure")
        .text()
        .await
        .expect("read detached failure stream");
    assert!(detached.contains("event: error"));
    assert!(detached.contains("detached failure"));
}

#[tokio::test]
async fn retries_terminal_context_window_errors_once_on_a_fresh_thread() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let recovered = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Context retry test",
            "messages":[{"role":"user","content":"CONTEXT_WINDOW_ONCE"}]
        }),
    )
    .await;
    assert_eq!(recovered["content"][0]["text"], "OK_AFTER_CONTEXT_RESTART");
}

#[tokio::test]
async fn bounds_context_window_retry_to_one_fresh_thread() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let failed = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "system":"Context retry bound",
            "messages":[{"role":"user","content":"CONTEXT_WINDOW_ALWAYS"}]
        }))
        .send()
        .await
        .expect("request bounded context retry");
    assert_eq!(failed.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(
        failed
            .text()
            .await
            .expect("read context retry failure")
            .contains("contextWindowExceeded")
    );
}

#[tokio::test]
async fn configured_collaborator_does_not_create_an_internal_tool() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let request = |model: &str| {
        json!({
            "model":"test-main-model", "claudex_collaborator_model":model,
            "system":"Collaborator bridge test",
            "messages":[{"role":"user","content":"USE_COLLABORATOR"}]
        })
    };
    let response = post_json(
        &client,
        &messages_url(&adapter),
        request("test-collaborator-model"),
    )
    .await;
    assert!(
        response["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("was not supplied by Claude Code and was not executed")
    );
}

#[tokio::test]
async fn rejects_unknown_tool_results_and_turn_errors() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let unknown = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "system":[], "tools":[{}],
            "messages":[{"role":"user","content":[{
                "type":"tool_result","tool_use_id":"unknown","content":"no match"
            }]}]
        }))
        .send()
        .await
        .expect("request expected bridge error");
    assert_eq!(unknown.status(), reqwest::StatusCode::BAD_GATEWAY);

    let turn_error = client
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-main-model", "system":"Turn error test",
            "messages":[{"role":"user","content":"TURN_ERROR"}]
        }))
        .send()
        .await
        .expect("request forced turn error");
    assert_eq!(turn_error.status(), reqwest::StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn configured_advisor_does_not_create_an_internal_tool() {
    let adapter = Adapter::start_with_models(Some("test-advisor-model"), None).await;
    let advisor = post_json(
        &Client::new(),
        &messages_url(&adapter),
        json!({
            "model":"test-main-model", "system":"Advisor bridge test",
            "messages":[{"role":"user","content":"USE_ADVISOR CURRENT_TURN_ADVISOR"}]
        }),
    )
    .await;
    let text = advisor["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("was not supplied by Claude Code and was not executed")
            || text.contains("advisor() is main-session only and was not executed"),
        "{text}"
    );
}

#[tokio::test]
async fn received_advisor_and_collaborator_schemas_are_public_tool_uses() {
    let adapter = Adapter::start_with_models(
        Some("must-not-run-advisor"),
        Some("must-not-run-collaborator"),
    )
    .await;
    let client = Client::new();
    for (prompt, name) in [
        ("USE_ADVISOR_PUBLIC", "advisor"),
        ("USE_COLLABORATOR_PUBLIC", "claude_collaborator"),
    ] {
        let response = post_json(
            &client,
            &messages_url(&adapter),
            json!({
                "model":"test-main-model",
                "system":"Public tool schema authority test",
                "tools":[{
                    "name":name,
                    "description":"public Claude Code tool",
                    "input_schema":{
                        "type":"object",
                        "properties":{
                            "key":{"type":"string"},
                            "task":{"type":"string"}
                        },
                        "additionalProperties":false
                    }
                }],
                "messages":[{"role":"user","content":prompt}]
            }),
        )
        .await;
        assert_eq!(response["stop_reason"], "tool_use");
        assert_eq!(response["content"].as_array().unwrap().len(), 1);
        assert_eq!(response["content"][0]["name"], name);
    }
}

#[tokio::test]
async fn selects_effort_independently_for_each_request() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let request = |system: &str, output_config: Value| {
        json!({
            "model":"test-main-model", "system":system, "output_config":output_config,
            "messages":[{"role":"user","content":"REPORT_EFFORT"}]
        })
    };
    let explicit = post_json(
        &client,
        &messages_url(&adapter),
        request("Explicit subagent effort", json!({"effort":"xhigh"})),
    )
    .await;
    let configured = post_json(
        &client,
        &messages_url(&adapter),
        request("Configured main effort", json!({})),
    )
    .await;
    assert_eq!(explicit["content"][0]["text"], "xhigh");
    assert_eq!(configured["content"][0]["text"], "medium");
}

#[tokio::test]
async fn routes_non_main_models_to_subscription_with_requested_effort() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let workspace = tempfile::tempdir().expect("create subscription workspace");
    let workspace = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let system = format!("<env>\nWorking directory: {}\n</env>", workspace.display());
    let expected = format!("test-sonnet-model|high|Read|Read|{}", workspace.display());
    assert_subscription_response(&client, &adapter, &system, &expected).await;
    assert_streaming_subscription(&client, &adapter, &system).await;
    assert_fast_subscription_outcomes(&client, &adapter, &system).await;
}

#[cfg(unix)]
fn structured_subscription_program(fixture: &tempfile::TempDir) -> std::path::PathBuf {
    let subscription_program = fixture.path().join("structured-subscription.sh");
    std::fs::write(
        &subscription_program,
        r#"#!/bin/sh
set -eu
trace="${0}.args"
: > "$trace"
for argument in "$@"; do
  printf '%s\n' "$argument" >> "$trace"
done
cat >/dev/null
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"UNSTRUCTURED_FALLBACK_MUST_NOT_LEAK","structured_output":{"answer":"STRUCTURED_SUBSCRIPTION_OK"}}'
"#,
    )
    .expect("write structured subscription fixture");
    let mut permissions = std::fs::metadata(&subscription_program)
        .expect("read structured subscription fixture metadata")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&subscription_program, permissions)
        .expect("make structured subscription fixture executable");
    subscription_program
}

#[cfg(unix)]
fn assert_structured_subscription_arguments(
    subscription_program: &std::path::Path,
    schema: &Value,
) {
    let arguments = std::fs::read_to_string(format!("{}.args", subscription_program.display()))
        .expect("read structured subscription arguments")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let expected = vec![
        "--safe-mode".to_owned(),
        "--tools".to_owned(),
        String::new(),
        "--allowedTools".to_owned(),
        String::new(),
        "--json-schema".to_owned(),
        schema.to_string(),
    ];
    assert!(
        arguments
            .windows(expected.len())
            .any(|window| window == expected.as_slice()),
        "structured subscription argv did not preserve explicit empty tools and schema: {arguments:?}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn routes_omitted_tool_compaction_through_safe_structured_subscription() {
    let fixture = tempfile::tempdir().expect("create structured subscription fixture");
    let codex_home = fixture.path().join(".codex");
    std::fs::create_dir(&codex_home).expect("create fixture CODEX_HOME");
    std::fs::write(
        codex_home.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write fixture Codex auth");
    let subscription_program = structured_subscription_program(&fixture);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind structured subscription adapter");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("structured subscription adapter address")
    );
    let app_server = claudex_agent_adapter::app_server::AppServer::spawn_with_program(
        "test-main-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &codex_home,
        &fixture.path().join("isolated-codex-home"),
    )
    .await
    .expect("start structured subscription mock app-server");
    let bridge = claudex_agent_adapter::anthropic::Bridge::new_with_subscription_program(
        app_server,
        "test-main-model".to_owned(),
        &subscription_program,
    )
    .with_settings_path(fixture.path().join("missing-claude-settings.json"));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            claudex_agent_adapter::http_router(
                std::sync::Arc::new(bridge),
                "test-main-model".to_owned(),
                None,
            ),
        )
        .await
        .expect("serve structured subscription adapter");
    });

    let schema = json!({
        "type":"object",
        "properties":{"answer":{"type":"string"}},
        "required":["answer"],
        "additionalProperties":false
    });
    let response = Client::new()
        .post(format!("{base_url}/v1/messages"))
        .json(&json!({
            "model":"test-sonnet-model", "max_tokens":256, "stream":false,
            "system":"Subscription structured output",
            "output_config":{"format":{"type":"json_schema","schema":schema.clone()}},
            "messages":[{"role":"user","content":concat!(
                "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n",
                "Your task is to create a detailed summary of the conversation so far"
            )}]
        }))
        .send()
        .await
        .expect("send structured subscription request")
        .error_for_status()
        .expect("successful structured subscription status")
        .json::<Value>()
        .await
        .expect("decode structured subscription response");

    assert_structured_subscription_arguments(&subscription_program, &schema);

    assert_eq!(response["content"][0]["type"], "text");
    assert_eq!(
        response["content"][0]["text"],
        r#"{"answer":"STRUCTURED_SUBSCRIPTION_OK"}"#
    );
    assert_eq!(response["stop_reason"], "end_turn");
    server.abort();
}

#[tokio::test]
async fn rejects_model_less_subscription_tools_before_forwarding_them() {
    let adapter = Adapter::start().await;
    let response = Client::new()
        .post(messages_url(&adapter))
        .json(&json!({
            "model":"test-sonnet-model", "stream":true,
            "system":"Parallel subscription tool bridge",
            "tools":[{
                "name":"Agent", "description":"Launch a worker",
                "input_schema":{
                    "type":"object",
                    "properties":{
                        "description":{"type":"string"},
                        "prompt":{"type":"string"},
                        "subagent_type":{"type":"string"}
                    }
                }
            }],
            "messages":[{"role":"user","content":"SUBSCRIPTION_PARALLEL_TOOLS"}]
        }))
        .send()
        .await
        .expect("request parallel subscription tools")
        .text()
        .await
        .expect("read parallel subscription tools");

    assert_eq!(response.matches(r#""name":"Agent""#).count(), 0);
    assert_eq!(response.matches("input_json_delta").count(), 0);
    assert!(response.contains("requested SubAgent model is not configured"));
    assert!(!response.contains("tool-alpha"));
    assert!(!response.contains("tool-beta"));
    assert!(!response.contains("INNER_TOOL_REJECTION_MUST_NOT_LEAK"));
    assert!(response.contains(r#""stop_reason":"end_turn""#));
}

#[tokio::test]
async fn subscription_follow_up_stream_distinguishes_launch_from_no_launch() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let request = |marker: &str| {
        json!({
            "model":"test-sonnet-model", "stream":true,
            "system":"Subscription follow-up visibility",
            "tools":[{
                "name":"Agent", "description":"Launch a worker",
                "input_schema":{"type":"object"}
            }],
            "messages":[
                {"role":"user","content":"start earlier work"},
                {"role":"assistant","content":[{
                    "type":"tool_use", "id":"prior-agent", "name":"Agent",
                    "input":{"subagent_type":"general-purpose"}
                }]},
                {"role":"user","content":[{
                    "type":"tool_result", "tool_use_id":"prior-agent", "content":"done"
                }]},
                {"role":"assistant","content":"earlier work complete"},
                {"role":"user","content":format!(
                    "{marker}\nClaudex routing for this turn: {}",
                    r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#
                )}
            ]
        })
    };

    let launch = client
        .post(messages_url(&adapter))
        .json(&request("SUBSCRIPTION_FOLLOW_UP_LAUNCH"))
        .send()
        .await
        .expect("request subscription launch follow-up")
        .text()
        .await
        .expect("read subscription launch follow-up");
    assert!(launch.contains(r#""name":"Agent""#));
    assert!(launch.contains(r#""stop_reason":"tool_use""#));
    assert!(!launch.contains("SubAgent status:"));

    let no_launch = client
        .post(messages_url(&adapter))
        .json(&request("SUBSCRIPTION_FOLLOW_UP_NO_LAUNCH"))
        .send()
        .await
        .expect("request subscription no-launch follow-up")
        .text()
        .await
        .expect("read subscription no-launch follow-up");
    assert!(no_launch.contains("SUBSCRIPTION_DIRECT_RESULT"));
    assert!(!no_launch.contains("SubAgent status:"));
    assert!(!no_launch.contains(r#""type":"tool_use""#));
    assert!(no_launch.contains(r#""stop_reason":"end_turn""#));
}

#[tokio::test]
async fn exchanges_large_subscription_input_and_output_without_pipe_deadlock() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let large_prompt = format!("SUBSCRIPTION_BACKPRESSURE{}", "x".repeat(128 * 1_024));
    for stream in [false, true] {
        let request = client.post(messages_url(&adapter)).json(&json!({
            "model":"test-backpressure-model",
            "stream":stream,
            "system":"Large subscription pipe test",
            "messages":[{"role":"user","content":large_prompt}]
        }));
        let response = tokio::time::timeout(Duration::from_secs(5), request.send())
            .await
            .expect("large subscription request deadlocked")
            .expect("send large subscription request")
            .error_for_status()
            .expect("successful large subscription status")
            .text()
            .await
            .expect("read large subscription response");
        assert!(response.contains("BACKPRESSURE_OK"), "response={response}");
        assert_subscription_stream_completion(&response, stream);
    }
}

async fn assert_subscription_response(
    client: &Client,
    adapter: &Adapter,
    system: &str,
    expected: &str,
) {
    let response = post_json(
        client,
        &messages_url(adapter),
        json!({
            "model":"test-sonnet-model", "system":system,
            "output_config":{"effort":"high"},
            "tools":[{"name":"Read","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"SUBSCRIPTION_ROUTE"}]
        }),
    )
    .await;
    assert_eq!(response["content"][0]["text"], expected);
}

async fn assert_streaming_subscription(client: &Client, adapter: &Adapter, system: &str) {
    let started = Instant::now();
    let mut response = client
        .post(messages_url(adapter))
        .json(&json!({
            "model":"test-sonnet-model", "stream":true,
            "system":system, "output_config":{"effort":"low"},
            "tools":[{"name":"Read","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"SUBSCRIPTION_STREAM_DELAY"}]
        }))
        .send()
        .await
        .expect("request subscription stream");
    let mut stream = String::new();
    while !stream.contains("STREAM_FIRST") {
        let chunk = response
            .chunk()
            .await
            .expect("read early subscription chunk")
            .expect("subscription stream ended before first delta");
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(started.elapsed() < Duration::from_millis(500));
    while let Some(chunk) = response.chunk().await.expect("read subscription remainder") {
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(stream.contains("STREAM_SECOND"));
    assert!(stream.contains("event: message_stop"));
    assert!(!stream.contains("Claudex is still working"));
    assert!(!stream.contains(r#""type":"thinking""#));
}

async fn assert_fast_subscription_outcomes(client: &Client, adapter: &Adapter, system: &str) {
    let empty_started = Instant::now();
    let empty = client
        .post(messages_url(adapter))
        .json(&json!({
            "model":"test-sonnet-model", "stream":true,
            "system":system,
            "messages":[{"role":"user","content":"SUBSCRIPTION_EMPTY"}]
        }))
        .send()
        .await
        .expect("request empty subscription stream")
        .text()
        .await
        .expect("read empty subscription stream");
    assert!(empty_started.elapsed() < Duration::from_millis(500));
    assert!(empty.contains(r#""text":"""#));
    assert!(empty.contains("event: message_stop"));
    assert!(!empty.contains("Claudex is still working"));
    assert!(!empty.contains(r#""type":"thinking""#));

    let failure_started = Instant::now();
    let failure = client
        .post(messages_url(adapter))
        .json(&json!({
            "model":"test-failing-model", "stream":true,
            "system":"Subscription failure stream",
            "messages":[{"role":"user","content":"SUBSCRIPTION_ROUTE"}]
        }))
        .send()
        .await
        .expect("request failing subscription stream")
        .text()
        .await
        .expect("read failing subscription stream");
    assert!(failure.contains("event: error"));
    assert!(
        failure.contains("forced subscription failure"),
        "unexpected subscription failure stream: {failure}"
    );
    assert!(failure_started.elapsed() < Duration::from_millis(500));
    assert!(!failure.contains("Claudex is still working"));
    assert!(!failure.contains(r#""type":"thinking""#));
}
