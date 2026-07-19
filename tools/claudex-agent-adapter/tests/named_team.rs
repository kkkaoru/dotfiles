mod support;

use reqwest::Client;
use serde_json::{Value, json};
use support::{Adapter, post_json};

fn tools() -> Value {
    json!([
        {
            "name":"Agent", "description":"Launch an agent",
            "input_schema":{"type":"object","properties":{
                "description":{"type":"string"}, "prompt":{"type":"string"},
                "subagent_type":{"type":"string"}, "run_in_background":{"type":"boolean"},
                "name":{"type":"string"}
            }}
        },
        {
            "name":"SendMessage", "description":"Message a teammate",
            "input_schema":{"type":"object","properties":{
                "to":{"type":"string"}, "summary":{"type":"string"},
                "message":{"type":"string"}
            }}
        },
        {
            "name":"TaskOutput", "description":"Read task output",
            "input_schema":{"type":"object","properties":{
                "task_id":{"type":"string"}
            }}
        }
    ])
}

fn request(messages: Value) -> Value {
    json!({
        "model":"test-main-model", "system":"Named teammate regression",
        "tools":tools(), "messages":messages
    })
}

fn result(call: &Value, content: &str) -> Value {
    json!({
        "role":"user", "content":[{
            "type":"tool_result", "tool_use_id":call["id"], "content":content
        }]
    })
}

#[tokio::test]
async fn routes_named_teammates_through_mailbox_instead_of_task_output() {
    let _ = Adapter::start_authenticated;
    let _ = support::base_request();
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let user = json!({
        "role":"user",
        "content":"USE_NAMED_TEAM_MAILBOX with the explicit teammate name company-profile"
    });

    let spawned = post_json(&client, &url, request(json!([user.clone()]))).await;
    let agent = &spawned["content"][0];
    assert_eq!(agent["name"], "Agent");
    assert_eq!(agent["input"]["name"], "company-profile");

    let metadata = "Spawned successfully. DELAY_NAMED_RESULT\nagent_id: company-profile@session-fixture\nname: company-profile\nThe agent is now running and will receive instructions via mailbox.";
    let original_request = request(json!([
        user.clone(),
        {"role":"assistant","content":spawned["content"]},
        result(agent, metadata)
    ]));
    let original_client = client.clone();
    let original_url = url.clone();
    let original =
        tokio::spawn(
            async move { post_json(&original_client, &original_url, original_request).await },
        );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let replay_request = request(json!([
        user.clone(),
        {"role":"assistant","content":spawned["content"]},
        {"role":"user","content":[
            {
                "type":"tool_result",
                "tool_use_id":agent["id"],
                "content":metadata
            },
            {"type":"text","text":"ASYNC_TEAM_NOTIFICATION"}
        ]}
    ]));
    let replay_client = client.clone();
    let replay_url = url.clone();
    let replay =
        tokio::spawn(async move { post_json(&replay_client, &replay_url, replay_request).await });
    let follow_up = original.await.expect("original result request");
    let coalesced = replay.await.expect("coalesced notification request");
    assert_eq!(coalesced["content"][0]["text"], "OK");

    assert_eq!(follow_up["content"][0]["name"], "SendMessage");
    assert_eq!(follow_up["content"][0]["input"]["to"], "company-profile");
    assert!(
        follow_up["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call["name"] != "TaskOutput")
    );

    let completed = post_json(
        &client,
        &url,
        request(json!([
            user.clone(),
            {"role":"assistant","content":spawned["content"]},
            result(agent, metadata),
            {"role":"assistant","content":follow_up["content"]},
            result(&follow_up["content"][0], "message delivered")
        ])),
    )
    .await;
    assert_eq!(
        completed["content"][0]["text"],
        "NAMED_TEAM_MAILBOX_COMPLETE"
    );
}
