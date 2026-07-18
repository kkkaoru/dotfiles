#[allow(dead_code)]
mod support;

use reqwest::Client;
use serde_json::json;
use support::{Adapter, post_json};

async fn launch_explicit_effort_agent(
    client: &Client,
    url: &str,
    user_id: &str,
    effort: &str,
    explicit_model: bool,
) -> String {
    let instruction = if explicit_model {
        "USE_AGENT_MODEL claude-opus-4-8"
    } else {
        "USE_AGENT"
    };
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

#[tokio::test]
async fn model_inferred_by_main_model_is_ignored_without_user_authorization() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user_id = r#"{"session_id":"inferred-model"}"#;
    let agent = post_json(
        &client,
        &url,
        json!({
            "model":"test-main-model", "system":"Agent routing regression test",
            "metadata":{"user_id":user_id},
            "tools":[{"name":"Agent","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"USE_AGENT_MODEL"}]
        }),
    )
    .await;
    let prompt = agent["content"][0]["input"]["prompt"]
        .as_str()
        .expect("decorated Agent prompt");
    let child = child_request(&client, &url, user_id, prompt, "claude-sonnet-5").await;
    assert_eq!(child["model"], "test-main-model");
    assert_eq!(child["content"][0]["text"], "medium");
}

#[tokio::test]
async fn arbitrary_explicit_agent_model_bypasses_native_enum_and_preserves_effort() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    for (requested, expected) in supported_efforts() {
        let user_id = format!(r#"{{"session_id":"subscription-{requested}"}}"#);
        let prompt = launch_explicit_effort_agent(&client, &url, &user_id, requested, true).await;
        let child = child_request(&client, &url, &user_id, &prompt, "test-sonnet-model").await;
        assert!(
            child["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with(&format!("claude-opus-4-8|{expected}|")))
        );
    }
}

#[tokio::test]
async fn agent_without_model_inherits_main_route_with_explicit_effort() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    for (requested, expected) in supported_efforts() {
        let user_id = format!(r#"{{"session_id":"app-server-{requested}"}}"#);
        let prompt = launch_explicit_effort_agent(&client, &url, &user_id, requested, false).await;
        let child = child_request(&client, &url, &user_id, &prompt, "test-sonnet-model").await;
        assert_eq!(child["content"][0]["text"], expected);
    }
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
async fn agent_without_effort_uses_configured_default_on_same_main_model() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user_id = r#"{"session_id":"same-main-default"}"#;
    let agent = post_json(
        &client,
        &url,
        json!({
            "model":"test-main-model", "system":"Agent default effort test",
            "output_config":{"effort":"low"}, "metadata":{"user_id":user_id},
            "tools":[{"name":"Agent","input_schema":{"type":"object"}}],
            "messages":[{"role":"user","content":"USE_AGENT_DEFAULT"}]
        }),
    )
    .await;
    assert!(agent["content"][0]["input"].get("effort").is_none());
    assert!(agent["content"][0]["input"].get("model").is_none());
    let prompt = agent["content"][0]["input"]["prompt"]
        .as_str()
        .expect("decorated default-effort prompt");
    let child = post_json(
        &client,
        &url,
        json!({
            "model":"test-sonnet-model",
            "system":[{"type":"text","text":"cc_is_subagent=true"}],
            "output_config":{"effort":"low"}, "metadata":{"user_id":user_id},
            "messages":[{"role":"user","content":prompt}]
        }),
    )
    .await;
    assert_eq!(child["content"][0]["text"], "medium");
}

#[tokio::test]
async fn unmatched_subagent_ignores_claude_codes_fallback_model() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    for (system, prompt) in [
        ("cc_is_subagent=true", "REPORT_EFFORT"),
        (
            "Claude Code current child request",
            "REPORT_EFFORT\n\n<claudex-agent-id>toolu_background</claudex-agent-id>",
        ),
    ] {
        let child = post_json(
            &client,
            &format!("{}/v1/messages", adapter.base_url),
            json!({
                "model":"claude-opus-4-8",
                "system":[{"type":"text","text":system}],
                "output_config":{"effort":"low"},
                "messages":[{"role":"user","content":prompt}]
            }),
        )
        .await;
        assert_eq!(child["model"], "test-main-model");
        assert_eq!(child["content"][0]["text"], "low");
    }
}
