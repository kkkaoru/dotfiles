mod support;

use reqwest::Client;
use serde_json::{Value, json};
use support::{Adapter, post_json};

fn tools() -> Value {
    json!([
        {
            "name":"Agent", "description":"Launch a subagent",
            "input_schema":{
                "type":"object",
                "properties":{
                    "description":{"type":"string"}, "prompt":{"type":"string"},
                    "subagent_type":{"type":"string"},
                    "run_in_background":{"type":"boolean"}
                },
                "required":["description","prompt","subagent_type"]
            }
        },
        {
            "name":"TaskOutput", "description":"Read a background task result",
            "input_schema":{
                "type":"object",
                "properties":{
                    "task_id":{"type":"string"}, "block":{"type":"boolean"},
                    "timeout":{"type":"integer"}
                },
                "required":["task_id"]
            }
        }
    ])
}

fn request(messages: Value) -> Value {
    json!({
        "model":"test-main-model", "max_tokens":256,
        "system":"Parallel Agent and TaskOutput regression", "tools":tools(),
        "messages":messages
    })
}

fn tool_results(response: &Value, values: &[&str]) -> Value {
    Value::Array(
        response["content"]
            .as_array()
            .expect("tool-use content")
            .iter()
            .zip(values)
            .map(|(block, value)| {
                json!({
                    "type":"tool_result", "tool_use_id":block["id"],
                    "content":value
                })
            })
            .collect(),
    )
}

#[tokio::test]
async fn preserves_parallel_agent_ids_for_follow_up_task_output_calls() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({"role":"user","content":"USE_PARALLEL_AGENTS_TASK_OUTPUT"});

    let agents = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(agents["stop_reason"], "tool_use");
    assert_eq!(agents["content"].as_array().unwrap().len(), 3);
    assert!(
        agents["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| { call["name"] == "Agent" && call["input"]["run_in_background"] == true })
    );

    let agent_ids = ["agent-profile-7", "agent-business-8", "agent-funding-9"];
    let agent_results = tool_results(&agents, &agent_ids);
    let outputs = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":agents["content"]},
            {"role":"user","content":agent_results}
        ])),
    )
    .await;
    assert_eq!(outputs["stop_reason"], "tool_use");
    let output_calls = outputs["content"].as_array().expect("TaskOutput calls");
    let returned_ids = output_calls
        .iter()
        .map(|call| call["input"]["task_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(returned_ids, agent_ids);
    assert!(output_calls.iter().all(|call| call["name"] == "TaskOutput"));

    let output_results = tool_results(&outputs, &["profile", "business", "funding"]);
    let completed = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":agents["content"]},
            {"role":"user","content":agent_results},
            {"role":"assistant","content":outputs["content"]},
            {"role":"user","content":output_results}
        ])),
    )
    .await;
    assert_eq!(
        completed["content"][0]["text"],
        "PARALLEL_AGENT_RESULTS_COMPLETE"
    );
    assert_eq!(completed["stop_reason"], "end_turn");
}
