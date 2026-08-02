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

async fn post_json_with_body_on_error(client: &Client, url: &str, body: Value) -> Value {
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("send JSON request");
    let status = response.status();
    let text = response.text().await.expect("read JSON response");
    assert!(status.is_success(), "HTTP {status}: {text}");
    serde_json::from_str(&text).expect("decode JSON response")
}

#[tokio::test]
async fn http_bridge_forwards_the_exact_agent_schema_without_a_batch_tool() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let response = post_json(
        &client,
        &url,
        json!({
            "model":"test-main-model",
            "max_tokens":256,
            "system":"Schema authority integration",
            "tools":[{
                "name":"Agent",
                "description":"native Agent",
                "input_schema":{
                    "type":"object",
                    "properties":{
                        "prompt":{"type":"string","description":"schema-authority-sentinel"},
                        "subagent_type":{"type":"string","enum":["general-purpose","Explore"]}
                    },
                    "required":["prompt"],
                    "additionalProperties":false,
                    "x-native-contract":{"version":220}
                }
            }],
            "messages":[{"role":"user","content":"VERIFY_AGENT_SCHEMA_AUTHORITY"}]
        }),
    )
    .await;

    assert_eq!(response["stop_reason"], "end_turn");
    assert_eq!(response["content"][0]["text"], "AGENT_SCHEMA_AUTHORITY_OK");
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

async fn completed_parallel_history(client: &Client, url: &str, user: Value) -> Value {
    let agents = post_json(client, url, request(json!([user.clone()]))).await;
    assert_eq!(agents["stop_reason"], "tool_use");
    let agent_results = tool_results(
        &agents,
        &["agent-profile-7", "agent-business-8", "agent-funding-9"],
    );
    let outputs = post_json(
        client,
        url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":agents["content"]},
            {"role":"user","content":agent_results}
        ])),
    )
    .await;
    assert_eq!(outputs["stop_reason"], "tool_use");
    let output_results = tool_results(&outputs, &["profile", "business", "funding"]);
    let completed = post_json(
        client,
        url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":agents["content"]},
            {"role":"user","content":agent_results},
            {"role":"assistant","content":outputs["content"]},
            {"role":"user","content":output_results}
        ])),
    )
    .await;
    assert_eq!(completed["stop_reason"], "end_turn");
    assert_eq!(
        completed["content"][0]["text"],
        "PARALLEL_AGENT_RESULTS_COMPLETE"
    );
    json!([
        user,
        {"role":"assistant","content":agents["content"]},
        {"role":"user","content":agent_results},
        {"role":"assistant","content":outputs["content"]},
        {"role":"user","content":output_results},
        {"role":"assistant","content":completed["content"]}
    ])
}

#[tokio::test]
async fn pure_async_agent_launch_results_return_control_without_task_output_relaunch() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":"USE_PARALLEL_AGENTS_TASK_OUTPUT with three independent synthetic scopes using test-main-model"
    });
    let agents_response = client
        .post(&url)
        .json(&request(json!([user.clone()])))
        .send()
        .await
        .expect("send native Agent launch request");
    let agents_status = agents_response.status();
    let agents_body = agents_response
        .text()
        .await
        .expect("read native Agent launch response");
    assert!(
        agents_status.is_success(),
        "agent launch status {agents_status}: {agents_body}"
    );
    let agents: Value =
        serde_json::from_str(&agents_body).expect("decode native Agent launch response");
    assert_eq!(agents["stop_reason"], "tool_use");
    assert_eq!(agents["content"].as_array().unwrap().len(), 3);

    let launch_results = Value::Array(
        agents["content"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, block)| {
                json!({
                    "type":"tool_result",
                    "tool_use_id":block["id"],
                    "content":[{"type":"text", "text":format!(
                        "Async agent launched successfully.\nagentId: internal-{index}\nThe agent is working in the background."
                    )}]
                })
            })
            .collect(),
    );
    let handoff_response = client
        .post(&url)
        .json(&request(json!([
            user,
            {"role":"assistant", "content":agents["content"]},
            {"role":"user", "content":launch_results}
        ])))
        .send()
        .await
        .expect("send async launch handoff");
    let status = handoff_response.status();
    let handoff_body = handoff_response
        .text()
        .await
        .expect("read async launch handoff response");
    assert!(
        status.is_success(),
        "handoff status {status}: {handoff_body}"
    );
    let handoff: Value =
        serde_json::from_str(&handoff_body).expect("decode async launch handoff response");

    assert_eq!(handoff["stop_reason"], "end_turn");
    assert_eq!(
        handoff["content"][0]["text"],
        "3 background agents launched; the main prompt is ready."
    );
    assert!(!handoff.to_string().contains("agent-message"));
    assert!(
        !handoff["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| { block.get("name").and_then(Value::as_str) == Some("TaskOutput") })
    );
}

#[tokio::test]
async fn accepts_main_input_after_background_handoff_and_keeps_task_retrievable() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":concat!(
            "USE_PARALLEL_AGENTS_TASK_OUTPUT with one independent background scope\n",
            "Claudex routing for this turn: ",
            r#"{"providers":{},"selected_agents":["general-purpose"],"selected_workers":[{"agent":"general-purpose","model":"test-main-model"}]}"#
        )
    });
    let launched =
        post_json_with_body_on_error(&client, &url, request(json!([user.clone()]))).await;
    assert_eq!(launched["stop_reason"], "tool_use");
    let launch_results = Value::Array(
        launched["content"]
            .as_array()
            .expect("background launch tools")
            .iter()
            .map(|block| {
                json!({
                    "type":"tool_result",
                    "tool_use_id":block["id"],
                    "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: internal-background\nThe agent is working in the background."}]
                })
            })
            .collect(),
    );
    let handoff = post_json_with_body_on_error(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":launch_results}
        ])),
    )
    .await;
    assert_eq!(handoff["stop_reason"], "end_turn");
    assert_eq!(
        handoff["content"][0]["text"],
        "3 background agents launched; the main prompt is ready."
    );
    assert!(!handoff.to_string().contains("agent-message"));

    // This is the standard Claude Code follow-up shape: the main user sends a
    // new instruction after the background launch turn has ended. It must not
    // be converted into TaskStop or wait for the worker to finish.
    let continued = post_json_with_body_on_error(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":launch_results},
            {"role":"assistant","content":handoff["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_CONTINUE: answer this new main-session instruction while the background worker keeps running"}
        ])),
    )
    .await;
    assert_eq!(continued["stop_reason"], "end_turn");
    assert_eq!(continued["content"][0]["text"], "MAIN_RESPONSE_CONTINUED");
    assert!(
        continued["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| { block.get("name").is_none() })
    );

    let task_output = post_json_with_body_on_error(
        &client,
        &url,
        request(json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":launch_results},
            {"role":"assistant","content":handoff["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_CONTINUE: answer this new main-session instruction while the background worker keeps running"},
            {"role":"assistant","content":continued["content"]},
            {"role":"user","content":"CONTROL_SUBAGENTS_TASK_OUTPUT: retrieve the background worker result now"}
        ])),
    )
    .await;
    assert_eq!(task_output["stop_reason"], "tool_use");
    assert_eq!(task_output["content"][0]["name"], "TaskOutput");
    assert_eq!(
        task_output["content"][0]["input"]["task_id"],
        "agent-profile-7"
    );
}

async fn stream_follow_up(client: &Client, url: &str, messages: Value) -> String {
    client
        .post(url)
        .json(&json!({
            "model":"test-main-model", "max_tokens":256, "stream":true,
            "system":"Follow-up SubAgent visibility regression", "tools":tools(),
            "messages":messages
        }))
        .send()
        .await
        .expect("send streamed follow-up")
        .text()
        .await
        .expect("read streamed follow-up")
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
async fn main_user_follow_up_reads_running_subagent_output_without_mailbox_injection() {
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
    assert_eq!(control["content"][0]["name"], "TaskOutput");
    assert_eq!(control["content"][0]["input"]["task_id"], "agent-profile-7");

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
async fn main_user_follow_up_updates_task_lifecycle_on_a_fresh_session() {
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
    assert_eq!(
        control["stop_reason"], "tool_use",
        "control response: {control}"
    );
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

#[tokio::test]
async fn streamed_follow_up_after_a_completed_batch_visibly_launches_a_new_subagent() {
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
    let mut history = completed_parallel_history(&client, &url, user).await;
    history.as_array_mut().expect("history array").push(json!({
        "role":"user",
        "content":"FOLLOW_UP_LAUNCH_AGENT: start the newly requested independent task"
    }));

    let stream = stream_follow_up(&client, &url, history).await;

    for visible_launch_fragment in [
        "event: message_start",
        "event: content_block_start",
        r#""name":"Agent""#,
        "follow-up implementation",
        "event: message_delta",
        r#""stop_reason":"tool_use""#,
        "event: message_stop",
    ] {
        assert!(
            stream.contains(visible_launch_fragment),
            "missing visible launch fragment {visible_launch_fragment}: {stream}"
        );
    }
}

#[tokio::test]
async fn streamed_related_follow_up_reads_an_active_subagent_without_mailbox_injection() {
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
    let stream = stream_follow_up(
        &client,
        &url,
        json!([
            user,
            {"role":"assistant","content":launched["content"]},
            {"role":"user","content":"FOLLOW_UP_REUSE_AGENT: extend the profile worker's current task"}
        ]),
    )
    .await;

    for visible_reuse_fragment in [
        "event: content_block_start",
        r#""name":"TaskOutput""#,
        "agent-profile-7",
        r#""stop_reason":"tool_use""#,
        "event: message_stop",
    ] {
        assert!(
            stream.contains(visible_reuse_fragment),
            "missing visible reuse fragment {visible_reuse_fragment}: {stream}"
        );
    }
    assert!(
        !stream.contains(r#""name":"SendMessage""#),
        "ordinary follow-up must not expose the mailbox tool: {stream}"
    );
}

#[tokio::test]
async fn streamed_follow_up_without_a_subagent_is_distinguishable_from_launch_or_reuse() {
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
    let mut history = completed_parallel_history(&client, &url, user).await;
    history.as_array_mut().expect("history array").push(json!({
        "role":"user",
        "content":"FOLLOW_UP_NO_AGENT: answer directly without delegating"
    }));

    let stream = stream_follow_up(&client, &url, history).await;

    assert!(
        stream.contains("FOLLOW_UP_NO_AGENT_LAUNCHED"),
        "stream={stream}"
    );
    assert!(!stream.contains("SubAgent status:"), "stream={stream}");
    assert!(
        stream.contains(r#""stop_reason":"end_turn""#),
        "stream={stream}"
    );
    assert!(
        !stream.contains(r#""type":"tool_use""#),
        "a no-launch follow-up must not look like a launch: {stream}"
    );
}
