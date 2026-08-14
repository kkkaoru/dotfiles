use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use claudex_agent_adapter::{anthropic::Bridge, app_server::AppServer, http_router};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

#[path = "support/coverage_profile.rs"]
mod coverage_profile;

const MAIN_PROVIDER_MODEL: &str = "gpt-5.6-luna";
const SESSION_USER_ID: &str = r#"{"session_id":"outer-model-authority"}"#;
const OUTER_MODELS: [&str; 3] = ["claude-opus-5[1m]", "claude-fable-5", "claude-sonnet-5"];

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn outer_models_keep_authority_across_a_long_continue() {
    let fixture = tempfile::tempdir().expect("create outer-model fixture");
    let source = fixture.path().join("provider-source");
    let isolated_home = fixture.path().join("provider-home");
    let trace = fixture.path().join("provider-requests.jsonl");
    let provider_program = fixture.path().join("provider-mock");
    let subscription_trace = fixture.path().join("subscription-models.txt");
    let subscription_program = fixture.path().join("subscription-mock");
    let settings = fixture.path().join("settings.json");
    fs::create_dir(&source).expect("create provider source home");
    fs::write(source.join("auth.json"), "{}").expect("write provider auth fixture");
    fs::write(&settings, r#"{"effortLevel":"high"}"#).expect("write Claude settings");
    write_provider_mock(&provider_program, &trace);
    write_subscription_mock(&subscription_program, &subscription_trace, fixture.path());

    let app = AppServer::spawn_with_program(
        MAIN_PROVIDER_MODEL,
        &provider_program,
        &source,
        &isolated_home,
    )
    .await
    .expect("start traceable provider");
    let bridge = Bridge::new_with_subscription_program(
        Arc::clone(&app),
        MAIN_PROVIDER_MODEL.to_owned(),
        &subscription_program,
    )
    .with_settings_path(settings);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind outer-model adapter");
    let url = format!(
        "http://{}/v1/messages",
        listener.local_addr().expect("outer-model listener address")
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            http_router(Arc::new(bridge), MAIN_PROVIDER_MODEL.to_owned(), None),
        )
        .await
        .expect("serve outer-model adapter");
    });
    let client = Client::new();

    assert_invalid_models_are_terminal(&client, &url).await;
    assert_eq!(provider_turn_count(&trace), 0);
    assert!(subscription_models(&subscription_trace).is_empty());

    let first_model = OUTER_MODELS[0];
    let first_response = post_success(&client, &url, outer_request(first_model)).await;
    assert_subscription_response(&first_response, first_model);
    assert_eq!(provider_turn_count(&trace), 0);
    assert_eq!(
        subscription_models(&subscription_trace),
        vec![first_model.to_owned()]
    );

    let provider_response = post_success(&client, &url, provider_request()).await;
    assert_eq!(provider_response["model"], MAIN_PROVIDER_MODEL);
    assert_eq!(provider_response["content"][0]["text"], "PROVIDER_ROUTE_OK");
    assert_eq!(provider_turn_count(&trace), 1);
    assert_eq!(
        subscription_models(&subscription_trace),
        vec![first_model.to_owned()]
    );

    for model in &OUTER_MODELS[1..] {
        let response = post_success(&client, &url, outer_request(model)).await;
        assert_subscription_response(&response, model);
        assert_eq!(provider_turn_count(&trace), 1);
    }
    let expected_subscription_models = OUTER_MODELS.map(str::to_owned);
    assert_eq!(
        subscription_models(&subscription_trace),
        expected_subscription_models
    );

    let _ = app.request("force/exit", json!({})).await;
    assert_unavailable_main_is_terminal(&client, &url, false).await;
    assert_unavailable_main_is_terminal(&client, &url, true).await;
    assert_eq!(provider_turn_count(&trace), 1);
    assert_eq!(
        subscription_models(&subscription_trace),
        expected_subscription_models
    );

    server.abort();
}

#[tokio::test]
async fn session_id_only_header_overrides_historical_child_markers_over_http() {
    let fixture = tempfile::tempdir().expect("create request-identity fixture");
    let source = fixture.path().join("provider-source");
    let isolated_home = fixture.path().join("provider-home");
    let trace = fixture.path().join("provider-requests.jsonl");
    let provider_program = fixture.path().join("provider-mock");
    fs::create_dir(&source).expect("create provider source home");
    fs::write(source.join("auth.json"), "{}").expect("write provider auth fixture");
    write_provider_mock(&provider_program, &trace);

    let app = AppServer::spawn_with_program(
        MAIN_PROVIDER_MODEL,
        &provider_program,
        &source,
        &isolated_home,
    )
    .await
    .expect("start traceable provider");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind request-identity adapter");
    let url = format!(
        "http://{}/v1/messages",
        listener.local_addr().expect("request-identity address")
    );
    let bridge = Bridge::new(Arc::clone(&app), MAIN_PROVIDER_MODEL.to_owned());
    let server = tokio::spawn(serve_request_identity_adapter(listener, bridge));

    let response = Client::new()
        .post(url)
        .header("x-claude-code-session-id", "main-session-only")
        .header("x-claudex-disabled-subagent-models", MAIN_PROVIDER_MODEL)
        .json(&json!({
            "model":MAIN_PROVIDER_MODEL,
            "max_tokens":64,
            "stream":false,
            "system":"retained system\ncc_is_subagent=true\n<claudex-agent-id>archived-system-child</claudex-agent-id>",
            "metadata":{"user_id":r#"{"session_id":"main-session-only"}"#},
            "messages":[
                {
                    "role":"assistant",
                    "content":[{
                        "type":"tool_use",
                        "id":"archived-child",
                        "name":"Agent",
                        "input":{"prompt":"old child\n<claudex-agent-id>archived-child</claudex-agent-id>"}
                    }]
                },
                {
                    "role":"user",
                    "content":[{
                        "type":"tool_result",
                        "tool_use_id":"archived-child",
                        "content":"old child completed"
                    }]
                },
                {"role":"user","content":"continue the main session"}
            ]
        }))
        .send()
        .await
        .expect("send session-only identity request")
        .error_for_status()
        .expect("session-only header must retain main authority")
        .json::<Value>()
        .await
        .expect("decode provider response");

    assert_eq!(response["model"], MAIN_PROVIDER_MODEL);
    assert_eq!(response["content"][0]["text"], "PROVIDER_ROUTE_OK");
    assert_eq!(provider_turn_count(&trace), 1);

    let _ = app.request("force/exit", json!({})).await;
    server.abort();
}

async fn serve_request_identity_adapter(listener: tokio::net::TcpListener, bridge: Bridge) {
    axum::serve(
        listener,
        http_router(Arc::new(bridge), MAIN_PROVIDER_MODEL.to_owned(), None),
    )
    .await
    .expect("serve request-identity adapter");
}

fn assert_subscription_response(response: &Value, model: &str) {
    assert_eq!(response["model"], model, "response rewrote outer model");
    let route = response["content"][0]["text"]
        .as_str()
        .expect("Claude subscription route trace");
    assert_eq!(
        route.split('|').next(),
        Some(model),
        "subscription process received a different model: {route}"
    );
}

fn outer_request(model: &str) -> Value {
    json!({
        "model":model,
        "max_tokens":256,
        "stream":false,
        "system":"Outer main session acceptance test\ncc_is_subagent=true\n<claudex-agent-id>archived-system-agent</claudex-agent-id>",
        "metadata":{"user_id":SESSION_USER_ID},
        "messages":[
            {
                "role":"user",
                "content":format!("SUBSCRIPTION_ROUTE\n{}", "a".repeat(200_000))
            },
            {
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"archived-agent",
                    "name":"Agent",
                    "input":{"prompt":"archived work\nclaudex_launch_id: archived-agent\n<claudex-agent-id>archived-agent</claudex-agent-id>"}
                }]
            },
            {
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"archived-agent",
                    "content":"b".repeat(200_000)
                }]
            },
            {
                "role":"assistant",
                "content":format!("retained answer {}", "c".repeat(200_000))
            },
            {"role":"user","content":"continue"}
        ]
    })
}

fn provider_request() -> Value {
    json!({
        "model":MAIN_PROVIDER_MODEL,
        "max_tokens":64,
        "stream":false,
        "metadata":{"user_id":SESSION_USER_ID},
        "messages":[{"role":"user","content":"provider control"}]
    })
}

async fn assert_invalid_models_are_terminal(client: &Client, url: &str) {
    for request in [
        json!({
            "max_tokens":64,
            "stream":false,
            "messages":[{"role":"user","content":"missing model"}]
        }),
        json!({
            "model":"",
            "max_tokens":64,
            "stream":false,
            "messages":[{"role":"user","content":"empty model"}]
        }),
    ] {
        let response = client
            .post(url)
            .json(&request)
            .send()
            .await
            .expect("send invalid model request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

async fn post_success(client: &Client, url: &str, request: Value) -> Value {
    client
        .post(url)
        .header("x-claude-code-session-id", "outer-model-authority")
        .json(&request)
        .send()
        .await
        .expect("send adapter request")
        .error_for_status()
        .expect("successful adapter response")
        .json()
        .await
        .expect("decode adapter response")
}

async fn assert_unavailable_main_is_terminal(client: &Client, url: &str, disabled: bool) {
    let mut request = client.post(url).json(&provider_request());
    if disabled {
        request = request.header("x-claudex-disabled-subagent-models", MAIN_PROVIDER_MODEL);
    }
    let response = request
        .send()
        .await
        .expect("send unavailable provider request");
    assert!(
        response.status().is_server_error(),
        "unavailable provider must fail without subscription fallback: {}",
        response.status()
    );
}

fn provider_turn_count(trace: &std::path::Path) -> usize {
    fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(r#""method":"turn/start""#))
        .count()
}

fn subscription_models(trace: &std::path::Path) -> Vec<String> {
    fs::read_to_string(trace)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_provider_mock(program: &std::path::Path, trace: &std::path::Path) {
    let script = r#"#!/bin/sh
trace='__TRACE__'
thread=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$trace"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"thread/start"'*)
      thread=$((thread + 1))
      printf '{"id":%s,"result":{"thread":{"id":"provider-%s"}}}\n' "$id" "$thread"
      ;;
    *'"method":"turn/start"'*)
      printf '{"id":%s,"result":{"turn":{"id":"turn"}}}\n' "$id"
      printf '{"method":"item/agentMessage/delta","params":{"threadId":"provider-%s","delta":"PROVIDER_ROUTE_OK"}}\n' "$thread"
      printf '{"method":"turn/completed","params":{"threadId":"provider-%s","turn":{"status":"completed"}}}\n' "$thread"
      ;;
    *'"method":"force/exit"'*)
      exit 0
      ;;
  esac
done
"#
    .replace("__TRACE__", &trace.display().to_string());
    write_executable(program, &script);
}

fn write_subscription_mock(
    program: &std::path::Path,
    trace: &std::path::Path,
    fixture_root: &std::path::Path,
) {
    let script = r#"#!/bin/sh
trace='__TRACE__'
model=missing
previous=
for argument in "$@"; do
  if [ "$previous" = "--model" ]; then
    model=$argument
    break
  fi
  previous=$argument
done
printf '%s\n' "$model" >> "$trace"
exec '__SUBSCRIPTION_MOCK__' "$@"
"#
    .replace("__TRACE__", &trace.display().to_string())
    .replace(
        "__SUBSCRIPTION_MOCK__",
        &coverage_profile::wrapped_program_string(fixture_root, env!("CARGO_BIN_EXE_claude-mock")),
    );
    write_executable(program, &script);
}

fn write_executable(program: &std::path::Path, contents: &str) {
    fs::write(program, contents).expect("write executable fixture");
    let mut permissions = fs::metadata(program)
        .expect("executable fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(program, permissions).expect("make fixture executable");
}
