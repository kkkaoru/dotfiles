#[allow(dead_code)]
mod support;

use reqwest::Client;
use serde_json::json;
use support::{Adapter, post_json};

#[tokio::test]
async fn evicts_the_oldest_idle_session_at_capacity() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    // Fill the current hard limit, then prove the next request reuses an idle permit.
    for index in 0..257 {
        let response = post_json(
            &client,
            &url,
            json!({
                "model":"test-main-model",
                "system":format!("capacity-session-{index}"),
                "messages":[{"role":"user","content":"Say OK"}]
            }),
        )
        .await;
        assert_eq!(response["content"][0]["text"], "OK");
    }
}

#[tokio::test]
async fn accepts_more_than_the_legacy_limit_while_tool_results_are_pending() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    for index in 0..65 {
        let response = post_json(
            &client,
            &url,
            json!({
                "model":"test-main-model",
                "system":format!("pending-capacity-session-{index}"),
                "tools":[{
                    "name":"lookup",
                    "input_schema":{"type":"object","properties":{}}
                }],
                "messages":[{"role":"user","content":"USE_TOOL"}]
            }),
        )
        .await;
        assert_eq!(response["stop_reason"], "tool_use", "session {index}");
    }
}
