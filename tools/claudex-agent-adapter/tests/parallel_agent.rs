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
        },
        {
            "name":"TaskStop", "description":"Stop a running background task",
            "input_schema":{
                "type":"object",
                "properties":{
                    "task_id":{"type":"string"}, "reason":{"type":"string"}
                },
                "required":["task_id"]
            }
        },
        {
            "name":"SendMessage", "description":"Send an instruction or follow-up request to a running teammate",
            "input_schema":{
                "type":"object",
                "properties":{
                    "to":{"type":"string"}, "summary":{"type":"string"},
                    "message":{"type":"string"}
                },
                "required":["to","summary","message"]
            }
        },
        {
            "name":"TaskUpdate", "description":"Update a running background task",
            "input_schema":{
                "type":"object",
                "properties":{
                    "task_id":{"type":"string"}, "status":{"type":"string"},
                    "description":{"type":"string"}
                },
                "required":["task_id","status"]
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
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

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

#[tokio::test]
async fn main_user_follow_up_controls_running_subagents_on_a_fresh_session() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

    let launched = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");
    assert_eq!(launched["content"].as_array().unwrap().len(), 3);

    let control = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_STOP: stop the profile worker now"}
        ])),
    )
    .await;
    assert_eq!(control["stop_reason"], "tool_use");
    assert_eq!(control["content"][0]["name"], "TaskStop");
    assert_eq!(control["content"][0]["input"]["task_id"], "agent-profile-7");

    let stopped = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_STOP: stop the profile worker now"},
            {"role":"assistant","content":control["content"]},
            {"role":"user","content":tool_results(&control, &["stop delivered"])}
        ])),
    )
    .await;
    assert_eq!(stopped["content"][0]["text"], "stop delivered");
    assert_eq!(stopped["stop_reason"], "end_turn");
}

#[tokio::test]
async fn main_user_follow_up_sends_message_to_running_subagent_on_a_fresh_session() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

    let launched = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");

    let control = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_SEND_MESSAGE: ask the profile worker for its current findings"}
        ])),
    )
    .await;
    assert_eq!(control["stop_reason"], "tool_use");
    assert_eq!(control["content"][0]["name"], "SendMessage");
    assert_eq!(control["content"][0]["input"]["to"], "agent-profile-7");
    assert_eq!(
        control["content"][0]["input"]["message"],
        "Return your current findings and continue the assigned work."
    );

    let completed = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_SEND_MESSAGE: ask the profile worker for its current findings"},
            {"role":"assistant","content":control["content"]},
            {"role":"user","content":tool_results(&control, &["message delivered"])}
        ])),
    )
    .await;
    assert_eq!(completed["content"][0]["text"], "message delivered");
    assert_eq!(completed["stop_reason"], "end_turn");
}

#[tokio::test]
async fn main_user_follow_up_updates_running_subagent_on_a_fresh_session() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

    let launched = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");

    let control = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_TASK_UPDATE: mark the profile worker as actively revising its report"}
        ])),
    )
    .await;
    assert_eq!(control["stop_reason"], "tool_use");
    assert_eq!(control["content"][0]["name"], "TaskUpdate");
    assert_eq!(control["content"][0]["input"]["task_id"], "agent-profile-7");
    assert_eq!(control["content"][0]["input"]["status"], "in_progress");

    let completed = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_TASK_UPDATE: mark the profile worker as actively revising its report"},
            {"role":"assistant","content":control["content"]},
            {"role":"user","content":tool_results(&control, &["task updated"])}
        ])),
    )
    .await;
    assert_eq!(completed["content"][0]["text"], "task updated");
    assert_eq!(completed["stop_reason"], "end_turn");
}

#[tokio::test]
async fn main_user_follow_up_requests_running_subagent_output_without_stopping_it() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

    let launched = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");

    let control = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_TASK_OUTPUT: return the profile worker's current response"}
        ])),
    )
    .await;
    assert_eq!(control["stop_reason"], "tool_use");
    assert_eq!(control["content"][0]["name"], "TaskOutput");
    assert_eq!(control["content"][0]["input"]["task_id"], "agent-profile-7");
    assert_eq!(control["content"][0]["input"]["block"], true);

    let completed = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_TASK_OUTPUT: return the profile worker's current response"},
            {"role":"assistant","content":control["content"]},
            {"role":"user","content":tool_results(&control, &["task result delivered"])}
        ])),
    )
    .await;
    assert_eq!(completed["content"][0]["text"], "task result delivered");
    assert_eq!(completed["stop_reason"], "end_turn");
}

#[tokio::test]
async fn main_user_follow_up_continues_without_forcing_a_subagent_control_tool() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#,
            " mandatory policy"
        )
    });

    let launched = post_json(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");

    let continued = post_json(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_CONTINUE: continue the main response while workers keep running"}
        ])),
    )
    .await;
    assert_eq!(continued["stop_reason"], "end_turn");
    assert_eq!(continued["content"][0]["type"], "text");
    assert_eq!(continued["content"][0]["text"], "MAIN_RESPONSE_CONTINUED");
    assert!(
        continued["content"]
            .as_array()
            .expect("continued content")
            .iter()
            .all(|block| block["type"] != "tool_use")
    );
}
