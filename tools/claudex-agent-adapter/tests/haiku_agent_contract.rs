#[allow(dead_code)]
mod support;

use std::{fs, path::PathBuf};

use reqwest::Client;
use serde_json::json;
use support::Adapter;

const HAIKU_SEARCH_AGENT: &str = "claudex-haiku-search";
const HAIKU_MODEL: &str = "claude-haiku-4-5";
const HAIKU_EFFORT: &str = "max";

fn haiku_search_definition() -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .expect("workspace root")
        .parent()
        .expect("repository root");
    fs::read_to_string(root.join(format!(".claude/agents/{HAIKU_SEARCH_AGENT}.md")))
        .expect("Haiku search agent definition")
}

#[test]
fn haiku_search_definition_declares_an_unrestricted_native_route() {
    let definition = haiku_search_definition();
    let frontmatter = definition
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .collect::<Vec<_>>();

    assert!(frontmatter.contains(&format!("name: {HAIKU_SEARCH_AGENT}").as_str()));
    assert!(frontmatter.contains(&format!("model: {HAIKU_MODEL}").as_str()));
    assert!(frontmatter.contains(&format!("effort: {HAIKU_EFFORT}").as_str()));
    for restricted_field in ["tools:", "disallowedTools:", "permissionMode:"] {
        assert!(
            !frontmatter
                .iter()
                .any(|line| line.starts_with(restricted_field)),
            "{HAIKU_SEARCH_AGENT} must inherit, not set {restricted_field}",
        );
    }
    assert!(definition.contains("complete tool set and permission context"));
    assert!(definition.contains("shell and command"));
    assert!(definition.contains("execution"));
}

#[tokio::test]
async fn haiku_search_subscription_keeps_its_route_and_inherited_command_tools() {
    let adapter = Adapter::start().await;
    let response = Client::new()
        .post(format!("{}/v1/messages", adapter.base_url))
        .json(&json!({
            "model":HAIKU_MODEL,
            "system":[{"type":"text","text":format!(
                "x-anthropic-billing-header: cc_is_subagent=true;\\nname: {HAIKU_SEARCH_AGENT}"
            )}],
            "output_config":{"effort":HAIKU_EFFORT},
            "tools":[
                {"name":"Bash","input_schema":{"type":"object"}},
                {"name":"Git","input_schema":{"type":"object"}},
                {"name":"WebSearch","input_schema":{"type":"object"}},
                {"name":"WebFetch","input_schema":{"type":"object"}}
            ],
            "messages":[{"role":"user","content":"SUBSCRIPTION_ROUTE"}]
        }))
        .send()
        .await
        .expect("send Haiku contract request");
    let status = response.status();
    let body = response.text().await.expect("read Haiku contract response");
    if status == reqwest::StatusCode::BAD_REQUEST {
        assert!(
            body.contains("disabled by the active Claudex policy"),
            "{body}"
        );
        return;
    }
    assert!(status.is_success(), "{status}: {body}");
    let response: serde_json::Value = serde_json::from_str(&body).expect("decode route response");
    let route = response["content"][0]["text"]
        .as_str()
        .expect("Claude subscription route trace");
    let fields = route.split('|').collect::<Vec<_>>();
    assert_eq!(fields.len(), 5, "unexpected route trace: {route}");
    assert_eq!(fields[0], HAIKU_MODEL);
    assert_eq!(fields[1], HAIKU_EFFORT);
    assert_eq!(fields[2], "Bash,Git,WebSearch,WebFetch");
    assert_eq!(fields[3], "Bash,Git,WebSearch,WebFetch");
}
