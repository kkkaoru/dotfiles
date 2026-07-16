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
    for index in 0..65 {
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
