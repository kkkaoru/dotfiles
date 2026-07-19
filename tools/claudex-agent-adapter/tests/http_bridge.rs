mod support;

use std::time::{Duration, Instant};

use reqwest::Client;
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
    assert_eq!(models["data"][0]["id"], "test-main-model");

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

    let continued = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"", "system":"Test system prompt",
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
    while !stream.contains("FIRST") {
        let chunk = response
            .chunk()
            .await
            .expect("read early stream chunk")
            .expect("stream ended before first delta");
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "first text delta was buffered until the turn completed"
    );
    while let Some(chunk) = response.chunk().await.expect("read stream remainder") {
        stream.push_str(&String::from_utf8_lossy(&chunk));
    }
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
    assert_eq!(first["stop_reason"], "tool_use");
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
    assert_eq!(first["stop_reason"], "tool_use");
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
    assert_eq!(parallel["stop_reason"], "tool_use");
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
    assert_eq!(response["stop_reason"], "tool_use");
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
async fn bridges_collaborator_success_and_failure() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let request = |model: &str| {
        json!({
            "model":"test-main-model", "claudex_collaborator_model":model,
            "system":"Collaborator bridge test",
            "messages":[{"role":"user","content":"USE_COLLABORATOR"}]
        })
    };
    let success = post_json(
        &client,
        &messages_url(&adapter),
        request("test-collaborator-model"),
    )
    .await;
    assert_eq!(success["content"][0]["text"], "MOCK_COLLABORATOR_RESULT");

    let failure = post_json(
        &client,
        &messages_url(&adapter),
        request("test-failing-model"),
    )
    .await;
    assert!(
        failure["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("forced subscription failure")
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
async fn bridges_configured_advisor_model() {
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
    assert_eq!(advisor["content"][0]["text"], "MOCK_ADVISOR_CURRENT_TURN");
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
    let response = post_json(
        &client,
        &messages_url(&adapter),
        json!({
            "model":"test-sonnet-model", "system":system,
            "output_config":{"effort":"high"},
            "tools":[{"name":"Read","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"SUBSCRIPTION_ROUTE"}]
        }),
    )
    .await;
    assert_eq!(
        response["content"][0]["text"],
        format!("test-sonnet-model|high|Read|Read|{}", workspace.display())
    );

    let started = Instant::now();
    let mut response = client
        .post(messages_url(&adapter))
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

    let failure = client
        .post(messages_url(&adapter))
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
    assert!(failure.contains("forced subscription failure"));
}
