#[allow(dead_code)]
mod support;

use reqwest::Client;
use serde_json::json;
use support::{Adapter, post_json};

const DISABLED_MODELS_HEADER: &str = "x-claudex-disabled-subagent-models";

async fn launch_explicit_effort_agent(
    client: &Client,
    url: &str,
    user_id: &str,
    effort: &str,
) -> String {
    let instruction = "USE_AGENT_MODEL claude-opus-4-8";
    let agent = post_json(
        client,
        url,
        json!({
            "model":"test-main-model", "system":"Agent effort test",
            "output_config":{"effort":"low"}, "metadata":{"user_id":user_id},
            "tools":[{
                "name":"Agent", "description":"Launch an agent",
                "input_schema":{"type":"object","properties":{
                    "prompt":{"type":"string"}, "effort":{"type":"string"}
                }}
            }],
            "messages":[{"role":"user","content":
                format!("{instruction} EFFORT_{}", effort.to_uppercase())}]
        }),
    )
    .await;
    assert_eq!(agent["content"][0]["name"], "Agent");
    assert!(agent["content"][0]["input"].get("claudex_effort").is_none());
    assert!(agent["content"][0]["input"].get("model").is_none());
    assert!(agent["content"][0]["input"].get("claudex_model").is_none());
    let correlated_prompt = agent["content"][0]["input"]["prompt"]
        .as_str()
        .expect("decorated Agent prompt");
    assert!(correlated_prompt.contains("<claudex-agent-id>toolu_"));
    correlated_prompt.to_owned()
}

async fn native_claude_launch_prompt(client: &Client, url: &str, instruction: &str) -> String {
    let response = client
        .post(url)
        .json(&json!({
            "model":"test-main-model", "system":"missing Agent model test",
            "tools":[{"name":"Agent","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":instruction}]
        }))
        .send()
        .await
        .expect("send native Claude Agent launch");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read native Claude Agent launch");
    assert!(status.is_success(), "{status}: {body}");
    let response = serde_json::from_str::<serde_json::Value>(&body)
        .expect("decode native Claude Agent launch");
    if response["content"][0]["name"] != "Agent" {
        assert!(
            response["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("disabled by policy"))
        );
        return String::new();
    }
    assert_eq!(response["content"][0]["name"], "Agent");
    response["content"][0]["input"]["prompt"]
        .as_str()
        .expect("decorated native Claude Agent prompt")
        .to_owned()
}

#[tokio::test]
async fn arbitrary_explicit_agent_model_bypasses_native_enum_and_preserves_effort() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    for (requested, expected) in supported_efforts() {
        let user_id = format!(r#"{{"session_id":"subscription-{requested}"}}"#);
        let prompt = launch_explicit_effort_agent(&client, &url, &user_id, requested).await;
        let child = child_request(&client, &url, &user_id, &prompt, "test-sonnet-model").await;
        assert!(
            child["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with(&format!("claude-opus-4-8|{expected}|")))
        );
    }
}

#[tokio::test]
async fn native_claude_agent_without_model_routes_to_haiku() {
    let adapter = Adapter::start().await;
    let url = format!("{}/v1/messages", adapter.base_url);
    let prompt = native_claude_launch_prompt(&Client::new(), &url, "USE_AGENT EFFORT_HIGH").await;
    if prompt.is_empty() {
        return;
    }
    let child = child_request(&Client::new(), &url, "", &prompt, "claude-sonnet-5").await;
    assert_eq!(child["model"], "claude-haiku-4-5");
}

#[tokio::test]
async fn inferred_model_without_user_authorization_is_rejected_before_launch() {
    let adapter = Adapter::start().await;
    let response = Client::new()
        .post(format!("{}/v1/messages", adapter.base_url))
        .json(&json!({
            "model":"test-main-model", "system":"inferred model test",
            "tools":[{"name":"Agent","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"USE_AGENT_MODEL"}]
        }))
        .send()
        .await
        .expect("send inferred model launch");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read inferred model rejection");
    if status == reqwest::StatusCode::BAD_GATEWAY {
        assert!(body.contains("does not match the exact route"), "{body}");
        return;
    }
    assert_eq!(status, reqwest::StatusCode::OK, "{body}");
    assert!(
        body.contains("was not started") || body.contains("not configured"),
        "unauthorized inferred model must not launch: {body}"
    );
}

fn supported_efforts() -> [(&'static str, &'static str); 6] {
    [
        ("low", "low"),
        ("mid", "medium"),
        ("medium", "medium"),
        ("high", "high"),
        ("xhigh", "xhigh"),
        ("max", "max"),
    ]
}

async fn child_request(
    client: &Client,
    url: &str,
    user_id: &str,
    prompt: &str,
    model: &str,
) -> serde_json::Value {
    let teammate_prompt = format!("<teammate-message>{prompt}</teammate-message>");
    post_json(
        client,
        url,
        json!({
            "model":model,
            "system":[{"type":"text","text":
                "x-anthropic-billing-header: cc_is_subagent=true;"}],
            "output_config":{"effort":"low"}, "metadata":{"user_id":user_id},
            "messages":[{"role":"user","content":[
                {"type":"text","text":"fixture context"},
                {"type":"text","text":teammate_prompt}
            ]}]
        }),
    )
    .await
}

#[tokio::test]
async fn native_claude_agent_without_effort_or_model_routes_to_haiku() {
    let adapter = Adapter::start().await;
    let url = format!("{}/v1/messages", adapter.base_url);
    let prompt = native_claude_launch_prompt(&Client::new(), &url, "USE_AGENT_DEFAULT").await;
    if prompt.is_empty() {
        return;
    }
    let child = child_request(&Client::new(), &url, "", &prompt, "claude-sonnet-5").await;
    assert_eq!(child["model"], "claude-haiku-4-5");
}

#[tokio::test]
async fn unmatched_subagent_uses_its_configured_provider_model() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    for (system, prompt) in [
        ("cc_is_subagent=true", "REPORT_EFFORT"),
        (
            "Claude Code current child request",
            "REPORT_EFFORT\n\n<claudex-agent-id>toolu_background</claudex-agent-id>",
        ),
    ] {
        let response = client
            .post(format!("{}/v1/messages", adapter.base_url))
            .json(&json!({
                "model":"test-main-model",
                "system":[{"type":"text","text":system}],
                "output_config":{"effort":"low"},
                "messages":[{"role":"user","content":prompt}]
            }))
            .send()
            .await
            .expect("send unmatched SubAgent request");
        assert!(response.status().is_success());
    }
}

#[tokio::test]
async fn unmatched_claude_subagent_routes_to_haiku() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let child = child_request(
        &client,
        &format!("{}/v1/messages", adapter.base_url),
        r#"{"session_id":"unmatched-native"}"#,
        "unmatched Claude child",
        "claude-sonnet-5",
    )
    .await;

    assert_eq!(child["model"], "claude-haiku-4-5");
}

#[tokio::test]
async fn unmatched_unknown_subagent_model_is_rejected_without_a_fallback() {
    let adapter = Adapter::start().await;
    let response = Client::new()
        .post(format!("{}/v1/messages", adapter.base_url))
        .json(&json!({
            "model":"unrouted-worker", "system":"cc_is_subagent=true",
            "messages":[{"role":"user","content":"child request"}]
        }))
        .send()
        .await
        .expect("send unknown SubAgent request");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("read unknown SubAgent rejection")
            .contains("does not have a recoverable configured route")
    );
}

#[tokio::test]
async fn terminal_policy_blocks_disabled_subagents_without_fallback() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);

    let main = client
        .post(&url)
        .header(DISABLED_MODELS_HEADER, "test-main-model")
        .json(&json!({
            "model":"test-main-model", "system":"main request",
            "messages":[{"role":"user","content":"Say OK"}]
        }))
        .send()
        .await
        .expect("send allowed main request");
    assert!(main.status().is_success());

    let (default_status, default_body) = denied_child_response(
        &client,
        &url,
        "test-main-model",
        "test-main-model",
        "REPORT_EFFORT",
    )
    .await;
    assert_eq!(default_status, 400);
    assert!(default_body.contains("disabled by the active Claudex policy"));

    let user_id = r#"{"session_id":"denied-explicit-model"}"#;
    let prompt = launch_explicit_effort_agent(&client, &url, user_id, "high").await;
    let (explicit_status, explicit_body) = denied_child_response(
        &client,
        &url,
        "claude-opus-4-8",
        "test-sonnet-model",
        &prompt,
    )
    .await;
    assert_eq!(explicit_status, 400);
    assert!(explicit_body.contains("disabled by the active Claudex policy"));

    let allowed_user_id = r#"{"session_id":"allowed-explicit-model"}"#;
    let allowed_prompt = launch_explicit_effort_agent(&client, &url, allowed_user_id, "high").await;
    let allowed = child_request(
        &client,
        &url,
        allowed_user_id,
        &allowed_prompt,
        "test-sonnet-model",
    )
    .await;
    assert!(
        allowed["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("claude-opus-4-8|high|"))
    );
}

async fn denied_child_response(
    client: &Client,
    url: &str,
    disabled_model: &str,
    requested_model: &str,
    prompt: &str,
) -> (u16, String) {
    let response = client
        .post(url)
        .header(DISABLED_MODELS_HEADER, disabled_model)
        .json(&json!({
            "model":requested_model,
            "system":[{"type":"text","text":"cc_is_subagent=true"}],
            "messages":[{"role":"user","content":prompt}]
        }))
        .send()
        .await
        .expect("send denied child request");
    let status = response.status().as_u16();
    let body = response.text().await.expect("read denied child response");
    (status, body)
}
