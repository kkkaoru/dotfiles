use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use serde_json::{Value, json};

use super::super::{ActiveTurn, PiGateway};
use crate::app_server::{ThreadEventDispatcher, ThreadEvents};

fn event(mut fields: Value) -> Value {
    let object = fields.as_object_mut().expect("event fields object");
    object.insert("version".to_owned(), json!(1));
    object.insert("id".to_owned(), json!("request"));
    fields
}

fn gateway() -> PiGateway {
    PiGateway {
        provider: "provider".to_owned(),
        model_id: "model".to_owned(),
        process: tokio::sync::Mutex::new(None),
        directory: PathBuf::new(),
        socket: PathBuf::new(),
        token: "token".to_owned(),
        events: Arc::new(ThreadEventDispatcher::default()),
        active: Arc::new(Mutex::new(HashMap::<String, ActiveTurn>::new())),
        pending_request_ids: Arc::new(Mutex::new(HashMap::new())),
        alive: AtomicBool::new(true),
    }
}

#[tokio::test]
async fn translates_text_thinking_tool_usage_and_terminal_events() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    feed_text_thinking_tool_turn(&gateway);
    assert_translated_tool_stream(&receiver).await;
}

fn feed_text_thinking_tool_turn(gateway: &PiGateway) {
    let mut state = super::EventTranslateState::default();
    feed_text_and_thinking(gateway, &mut state);
    feed_tool_and_done(gateway, &mut state);
}

fn feed_text_and_thinking(gateway: &PiGateway, state: &mut super::EventTranslateState) {
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({"type":"text_delta","index":0,"delta":"hello"})),
                state
            )
            .expect("text delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({"type":"thinking_progress","index":1,"deltaChars":5})),
                state
            )
            .expect("thinking progress")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({"type":"thinking_result","index":1,"result":"ready"})),
                state
            )
            .expect("thinking result")
    );
}

fn feed_tool_and_done(gateway: &PiGateway, state: &mut super::EventTranslateState) {
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_start","index":2,"toolCallId":"call-1","name":"Read"
                })),
                state
            )
            .expect("tool start")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_delta","index":2,"delta":"{\"path\":\"a\"}"
                })),
                state
            )
            .expect("tool delta")
    );
    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_end","index":2,"toolCallId":"call-1","name":"Read",
                    "arguments":{"path":"a"}
                })),
                state
            )
            .expect("tool end")
    );
    assert!(
        gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"done","reason":"toolUse","message":{"usage":{
                        "input":11,"output":7,"reasoning":4,
                        "cacheRead":5,"cacheWrite":3,"cacheWrite1h":2,
                        "totalTokens":26,
                        "cost":{"input":1.0,"output":2.0,"cacheRead":0.1,
                            "cacheWrite":0.2,"total":3.3}
                    }}
                })),
                state
            )
            .expect("done")
    );
}

async fn assert_translated_tool_stream(receiver: &ThreadEvents) {
    let text = receiver.recv().await.expect("text event");
    let thinking_progress = receiver.recv().await.expect("thinking progress");
    let thinking = receiver.recv().await.expect("thinking result");
    let thinking_complete = receiver.recv().await.expect("thinking complete");
    let start = receiver.recv().await.expect("tool start");
    let delta = receiver.recv().await.expect("tool delta");
    let tool = receiver.recv().await.expect("tool event");
    let usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_translated_content(&text, &thinking_progress, &thinking, &thinking_complete);
    assert_translated_tool(&start, &delta, &tool);
    assert_translated_usage_and_done(&usage, &done);
}

fn assert_translated_content(
    text: &Value,
    thinking_progress: &Value,
    thinking: &Value,
    thinking_complete: &Value,
) {
    assert_eq!(text["method"], "item/agentMessage/delta");
    assert_eq!(thinking_progress["method"], "item/reasoning/progress");
    assert_eq!(thinking_progress["params"]["deltaChars"], 5);
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["delta"], "ready");
    assert_eq!(thinking["params"]["summaryIndex"], 0);
    assert_eq!(thinking_complete["method"], "item/reasoning/complete");
}

fn assert_translated_tool(start: &Value, delta: &Value, tool: &Value) {
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["tool"], "Read");
    assert_eq!(start["params"]["callId"], "call-1");
    assert_eq!(delta["method"], "item/tool/delta");
    assert_eq!(delta["params"]["delta"], "{\"path\":\"a\"}");
    assert_eq!(tool["params"]["arguments"], json!({"path":"a"}));
}

fn assert_translated_usage_and_done(usage: &Value, done: &Value) {
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["inputTokens"], 11);
    assert_eq!(usage["outputTokens"], 3);
    assert_eq!(usage["reasoningOutputTokens"], 4);
    assert_eq!(
        usage["outputTokens"].as_u64().expect("output")
            + usage["reasoningOutputTokens"].as_u64().expect("reasoning"),
        7
    );
    assert_eq!(usage["cacheReadInputTokens"], 5);
    assert_eq!(usage["cacheCreationInputTokens"], 3);
    assert_eq!(usage["cacheCreation1hInputTokens"], 2);
    assert_eq!(usage["totalTokens"], 26);
    assert_eq!(usage["cost"]["total"], 3.3);
    assert_eq!(done["params"]["turn"]["status"], "completed");
    assert_eq!(done["params"]["turn"]["providerStopReason"], "tool_use");
    assert_eq!(
        done["params"]["turn"]["terminal"],
        json!({"state":"complete"})
    );
}

#[tokio::test]
async fn toolcall_start_emits_live_progress_before_arguments_complete() {
    // Pi streamSimple emits toolcall_start as soon as grok begins a tool.
    // SubAgent TUI paints native tool_use cards from item/tool/start, not
    // ACP item/providerTool ▶ thinking. Buffering until toolcall_end drops
    // mid-run progress.
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    assert!(
        !gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"toolcall_start","index":0,"toolCallId":"call-1","name":"Read"
                })),
                &mut state
            )
            .expect("tool start")
    );

    let start = tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv())
        .await
        .expect("toolcall_start must dispatch live tool start, not wait for toolcall_end")
        .expect("start event");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["callId"], "call-1");
    assert_eq!(start["params"]["tool"], "Read");
    assert_ne!(start["method"], "item/providerTool/call");
}

#[tokio::test]
async fn toolcall_progress_waits_for_name_then_emits_once() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({"type":"toolcall_start","index":0,"toolCallId":"call-late"})),
            &mut state,
        )
        .expect("start without name");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "progress must wait until the tool name arrives"
    );

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"name":"Bash","delta":"{"
            })),
            &mut state,
        )
        .expect("name arrives on first delta");
    let start = receiver.recv().await.expect("late name start");
    let first_delta = receiver.recv().await.expect("first argument delta");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["callId"], "call-late");
    assert_eq!(start["params"]["tool"], "Bash");
    assert_eq!(first_delta["method"], "item/tool/delta");
    assert_eq!(first_delta["params"]["delta"], "{");

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"name":"Bash","delta":"}"
            })),
            &mut state,
        )
        .expect("later delta");
    let second_delta = receiver.recv().await.expect("later argument delta");
    assert_eq!(second_delta["method"], "item/tool/delta");
    assert_eq!(second_delta["params"]["delta"], "}");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "the same tool must not emit a second start card"
    );
}

#[tokio::test]
async fn toolcall_end_uses_buffered_bash_command_when_event_arguments_are_empty() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-bash","name":"Bash"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"delta":"{\"command\":\"ls -la\"}"
            })),
            &mut state,
        )
        .expect("delta");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-bash","name":"Bash",
                "arguments":{}
            })),
            &mut state,
        )
        .expect("end");
    let start = receiver.recv().await.expect("start event");
    let delta = receiver.recv().await.expect("delta event");
    let call = receiver.recv().await.expect("call event");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(delta["method"], "item/tool/delta");
    assert_eq!(call["method"], "item/tool/call");
    assert_eq!(call["params"]["arguments"]["command"], "ls -la");
}

#[tokio::test]
async fn normalized_thinking_result_and_delta_less_text_end_stream_once() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_result","index":0,"result":"plan the edit"
            })),
            &mut state,
        )
        .expect("thinking_result");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"text_end","index":1,"content":"done"
            })),
            &mut state,
        )
        .expect("text_end");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_result","index":0,"result":"plan the edit"
            })),
            &mut state,
        )
        .expect("duplicate thinking_result");

    let thinking = receiver.recv().await.expect("thinking from end");
    let complete = receiver.recv().await.expect("thinking complete");
    let text = receiver.recv().await.expect("text from end");
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["delta"], "plan the edit");
    assert_eq!(complete["method"], "item/reasoning/complete");
    assert_eq!(text["method"], "item/agentMessage/delta");
    assert_eq!(text["params"]["delta"], "done");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "a later thinking_result must not replay or complete twice"
    );
}

#[tokio::test]
async fn legacy_thinking_hides_raw_deltas_and_emits_only_the_end_result() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();

    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({"type":"thinking_start","index":3})),
            &mut state,
        )
        .expect("thinking_start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_delta","index":3,"delta":"why"
            })),
            &mut state,
        )
        .expect("thinking_delta");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"thinking_end","index":3,
                "content":"private reasoning\n\nThe compatibility result."
            })),
            &mut state,
        )
        .expect("thinking_end after delta");

    let start = receiver.recv().await.expect("start progress");
    let delta = receiver.recv().await.expect("delta progress");
    let thinking = receiver.recv().await.expect("result thinking");
    let complete = receiver.recv().await.expect("thinking complete");
    assert_eq!(start["method"], "item/reasoning/progress");
    assert_eq!(start["params"]["deltaChars"], 0);
    assert_eq!(delta["method"], "item/reasoning/progress");
    assert_eq!(delta["params"]["deltaChars"], 0);
    assert_eq!(thinking["method"], "item/reasoning/summaryTextDelta");
    assert_eq!(thinking["params"]["delta"], "The compatibility result.");
    assert_ne!(thinking["params"]["delta"], "why");
    assert_eq!(complete["method"], "item/reasoning/complete");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "legacy thinking must emit no extra raw content"
    );
}

#[tokio::test]
async fn omits_unreported_one_hour_cache_usage() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"done","reason":"stop",
                "message":{"usage":{"input":1,"output":2,"cacheWrite":3}}
            })),
            &mut super::EventTranslateState::default(),
        )
        .expect("done");

    let usage = receiver.recv().await.expect("usage event");
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["cacheCreationInputTokens"], 3);
    assert!(usage.get("cacheCreation1hInputTokens").is_none());
}

#[tokio::test]
async fn clamps_invalid_usage_subsets_without_underflow() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"done","reason":"stop",
                "message":{"usage":{
                    "output":3,"reasoning":5,"cacheWrite":2,"cacheWrite1h":4
                }}
            })),
            &mut super::EventTranslateState::default(),
        )
        .expect("done");

    let usage = receiver.recv().await.expect("usage event");
    let usage = &usage["params"]["tokenUsage"]["last"];
    assert_eq!(usage["outputTokens"], 0);
    assert_eq!(usage["reasoningOutputTokens"], 3);
    assert_eq!(usage["cacheCreationInputTokens"], 2);
    assert_eq!(usage["cacheCreation1hInputTokens"], 2);
}

#[test]
fn maps_only_supported_pi_stop_reasons() {
    assert_eq!(
        super::anthropic_stop_reason(&json!({"reason":"stop"})).expect("supported reason"),
        "end_turn"
    );
    assert_eq!(
        super::anthropic_stop_reason(&json!({"reason":"length"})).expect("supported reason"),
        "max_tokens"
    );
    assert_eq!(
        super::anthropic_stop_reason(&json!({"reason":"toolUse"})).expect("supported reason"),
        "tool_use"
    );
    assert_eq!(
        super::anthropic_stop_reason(&json!({"reason":"deferred"})).expect("supported reason"),
        "pause_turn"
    );
    assert!(super::anthropic_stop_reason(&json!({"reason":"pending"})).is_err());
    assert!(super::anthropic_stop_reason(&json!({})).is_err());
}

#[test]
fn maps_spawn_subagent_launch_to_agent() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({"prompt":"CHILD_OK","description":"smoke"}),
        &HashSet::new(),
    )
    .expect("spawn launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({"prompt":"CHILD_OK","description":"smoke","run_in_background":true})
    );
}

#[test]
fn maps_prefixed_spawn_subagent_launch_to_agent() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "MCP__spawn_subagent",
        json!({"prompt":"CHILD_OK"}),
        &HashSet::new(),
    )
    .expect("prefixed spawn launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({"prompt":"CHILD_OK","run_in_background":true})
    );
}

#[test]
fn leaves_agent_launch_without_resume() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({"prompt":"CHILD_OK","description":"smoke"}),
        &HashSet::new(),
    )
    .expect("agent launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({"prompt":"CHILD_OK","description":"smoke"})
    );
}

#[test]
fn maps_spawn_subagent_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "prompt":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_agent_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({
            "prompt":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_agent_resume_alias_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({
            "prompt":"continue the review",
            "resume":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_task_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Task",
        json!({
            "prompt":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_agent_to_and_prompt_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({
            "prompt":"continue the review",
            "to":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_message_field_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "message":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_task_field_as_continue_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "task":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn normalizes_existing_send_message_to_to_and_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "SendMessage",
        json!({
            "to":"a0123456789abcdef0",
            "message":"continue the review",
            "extra":"drop-me"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn strips_empty_resume_from_on_spawn_subagent_launch() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({"prompt":"CHILD_OK","resume_from":""}),
        &HashSet::new(),
    )
    .expect("empty resume launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({"prompt":"CHILD_OK","run_in_background":true})
    );
}

#[test]
fn strips_whitespace_resume_from_on_agent_launch() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({"prompt":"CHILD_OK","resume_from":"   "}),
        &HashSet::new(),
    )
    .expect("whitespace resume launch");
    assert_eq!(name, "Agent");
    assert_eq!(arguments, json!({"prompt":"CHILD_OK"}));
}

#[test]
fn strips_resume_from_without_prompt_from_spawn_subagent() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({"resume_from":"a0123456789abcdef0"}),
        &HashSet::new(),
    )
    .expect("resume without prompt");
    assert_eq!(name, "Agent");
    assert_eq!(arguments, json!({"run_in_background":true}));
}

#[test]
fn remaps_grok_medium_spawn_subagent_type() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "prompt":"CHILD_OK",
            "subagent_type":"grok-native-medium-plugin-v3:claudex-medium"
        }),
        &HashSet::new(),
    )
    .expect("grok medium launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({
            "prompt":"CHILD_OK",
            "subagent_type":"claudex-grok",
            "run_in_background":true
        })
    );
}

#[test]
fn remaps_claudex_medium_suffix_spawn_subagent_type() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "prompt":"CHILD_OK",
            "subagent_type":"custom-worker:claudex-medium"
        }),
        &HashSet::new(),
    )
    .expect("medium suffix launch");
    assert_eq!(name, "Agent");
    assert_eq!(arguments["subagent_type"], "claudex-grok");
    assert_eq!(arguments["run_in_background"], true);
}

#[test]
fn keeps_other_spawn_subagent_types() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "prompt":"CHILD_OK",
            "subagent_type":"claudex-qwen"
        }),
        &HashSet::new(),
    )
    .expect("other spawn type");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({
            "prompt":"CHILD_OK",
            "subagent_type":"claudex-qwen",
            "run_in_background":true
        })
    );
}

#[test]
fn strips_provider_only_spawn_subagent_fields() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "prompt":"CHILD_OK",
            "cwd":"/tmp/assigned-worktree",
            "background":false,
            "capability_mode":"all"
        }),
        &HashSet::new(),
    )
    .expect("provider fields launch");
    assert_eq!(name, "Agent");
    assert_eq!(
        arguments,
        json!({"prompt":"CHILD_OK","run_in_background":true})
    );
}

#[test]
fn maps_instruction_alias_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!({
            "instruction":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_query_alias_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Agent",
        json!({
            "query":"continue the review",
            "resume_from":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn maps_input_alias_resume_from_to_send_message() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Task",
        json!({
            "input":"continue the review",
            "to":"a0123456789abcdef0"
        }),
        &HashSet::from(["SendMessage".to_owned()]),
    )
    .expect("listed SendMessage");
    assert_eq!(name, "SendMessage");
    assert_eq!(
        arguments,
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[test]
fn leaves_non_object_agent_arguments() {
    let (name, arguments) =
        super::events_finish::mapped_claude_code_tool("Agent", json!(3), &HashSet::new())
            .expect("non-object agent");
    assert_eq!(name, "Agent");
    assert_eq!(arguments, json!(3));
}

#[test]
fn wraps_non_object_spawn_subagent_arguments() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "spawn_subagent",
        json!("go"),
        &HashSet::new(),
    )
    .expect("non-object spawn");
    assert_eq!(name, "Agent");
    assert_eq!(arguments, json!({"value":"go","run_in_background":true}));
}

#[test]
fn leaves_read_tool_with_resume_from() {
    let (name, arguments) = super::events_finish::mapped_claude_code_tool(
        "Read",
        json!({"path":"CLAUDE.md","resume_from":"a0123456789abcdef0"}),
        &HashSet::new(),
    )
    .expect("read passthrough");
    assert_eq!(name, "Read");
    assert_eq!(
        arguments,
        json!({"path":"CLAUDE.md","resume_from":"a0123456789abcdef0"})
    );
}

#[test]
fn fail_closed_spawn_resume_without_listed_send_message() {
    assert!(
        super::events_finish::mapped_claude_code_tool(
            "spawn_subagent",
            json!({
                "prompt":"continue the review",
                "resume_from":"a0123456789abcdef0"
            }),
            &HashSet::new(),
        )
        .is_none()
    );
}

#[test]
fn fail_closed_agent_resume_when_only_agent_listed() {
    assert!(
        super::events_finish::mapped_claude_code_tool(
            "Agent",
            json!({
                "prompt":"continue the review",
                "resume_from":"a0123456789abcdef0"
            }),
            &HashSet::from(["Agent".to_owned()]),
        )
        .is_none()
    );
}

#[test]
fn listed_claude_tool_names_reads_original_send_message() {
    let names = super::events_finish::listed_claude_tool_names(&json!({
        "tools":[
            {"name":"Agent"},
            {"name":"SendMessage"},
            {"name":"Bash"},
            {"name":""},
            {"name":"  "}
        ]
    }));
    assert!(names.contains("SendMessage"));
    assert!(names.contains("Agent"));
    assert!(names.contains("Bash"));
    assert!(!names.contains(""));
}

#[test]
fn listed_claude_tool_names_empty_without_tools_array() {
    let names = super::events_finish::listed_claude_tool_names(&json!({"messages":[]}));
    assert!(names.is_empty());
}

#[test]
fn from_request_tools_lists_send_message_from_pi_request() {
    let state = super::event_translate_state(&json!({
        "tools":[{"name":"Agent"},{"name":"SendMessage"}]
    }));
    assert!(state.listed_tools.contains("SendMessage"));
    assert!(state.listed_tools.contains("Agent"));
}

#[test]
fn bash_cmd_alias_from_event_is_usable() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "Bash".to_owned(),
        arguments: String::new(),
        start_emitted: true,
    };
    let value = super::events_finish::finished_tool_arguments(
        &json!({"arguments":{"cmd":"ls -la"}}),
        &tool,
    )
    .expect("cmd alias");
    assert_eq!(value, json!({"cmd":"ls -la"}));
}

#[test]
fn bash_script_alias_from_event_is_usable() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "Bash".to_owned(),
        arguments: String::new(),
        start_emitted: true,
    };
    let value = super::events_finish::finished_tool_arguments(
        &json!({"arguments":{"script":"ls -la"}}),
        &tool,
    )
    .expect("script alias");
    assert_eq!(value, json!({"script":"ls -la"}));
}

#[test]
fn bash_field_alias_from_event_is_usable() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "Bash".to_owned(),
        arguments: String::new(),
        start_emitted: true,
    };
    let value = super::events_finish::finished_tool_arguments(
        &json!({"arguments":{"bash":"ls -la"}}),
        &tool,
    )
    .expect("bash alias");
    assert_eq!(value, json!({"bash":"ls -la"}));
}

#[test]
fn non_object_event_arguments_fall_back_to_bash_buffer() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "Bash".to_owned(),
        arguments: r#"{"command":"ls -la"}"#.to_owned(),
        start_emitted: true,
    };
    let value =
        super::events_finish::finished_tool_arguments(&json!({"arguments":"not-an-object"}), &tool)
            .expect("buffer");
    assert_eq!(value, json!({"command":"ls -la"}));
}

#[test]
fn invalid_buffered_arguments_without_event_args_error() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "Read".to_owned(),
        arguments: "{".to_owned(),
        start_emitted: true,
    };
    assert!(
        super::events_finish::finished_tool_arguments(&json!({}), &tool).is_err(),
        "invalid buffered JSON must fail when the event omitted arguments"
    );
}

#[test]
fn nested_tool_call_arguments_are_used_for_spawn_resume() {
    let tool = super::ToolCallBuffer {
        id: "id".to_owned(),
        name: "spawn_subagent".to_owned(),
        arguments: String::new(),
        start_emitted: true,
    };
    let value = super::events_finish::finished_tool_arguments(
        &json!({
            "toolCall":{
                "arguments":{
                    "prompt":"continue the review",
                    "resume_from":"a0123456789abcdef0"
                }
            }
        }),
        &tool,
    )
    .expect("nested toolCall arguments");
    assert_eq!(
        value,
        json!({
            "prompt":"continue the review",
            "resume_from":"a0123456789abcdef0"
        })
    );
}

#[test]
fn mapped_start_name_rewrites_spawn_subagent_to_agent() {
    assert_eq!(
        super::events_finish::mapped_start_tool_name("spawn_subagent"),
        "Agent"
    );
    assert_eq!(
        super::events_finish::mapped_start_tool_name("MCP__spawn_subagent"),
        "Agent"
    );
    assert_eq!(super::events_finish::mapped_start_tool_name("Bash"), "Bash");
    assert_eq!(
        super::events_finish::mapped_start_tool_name("Agent"),
        "Agent"
    );
}

#[tokio::test]
async fn spawn_subagent_launch_emits_agent_tool_call() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-spawn","name":"spawn_subagent"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"delta":"{\"prompt\":\"CHILD_OK\"}"
            })),
            &mut state,
        )
        .expect("delta");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-spawn","name":"spawn_subagent",
                "arguments":{"prompt":"CHILD_OK","description":"smoke"}
            })),
            &mut state,
        )
        .expect("end");
    let start = receiver.recv().await.expect("start event");
    let delta = receiver.recv().await.expect("delta event");
    let call = receiver.recv().await.expect("call event");
    assert_eq!(start["method"], "item/tool/start");
    assert_eq!(start["params"]["tool"], "Agent");
    assert_eq!(delta["method"], "item/tool/delta");
    assert_eq!(call["method"], "item/tool/call");
    assert_eq!(call["params"]["tool"], "Agent");
    assert_eq!(
        call["params"]["arguments"],
        json!({"prompt":"CHILD_OK","description":"smoke","run_in_background":true})
    );
}

#[tokio::test]
async fn spawn_subagent_resume_from_emits_send_message() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::event_translate_state(&json!({
        "tools":[{"name":"Agent"},{"name":"SendMessage"}]
    }));
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-resume","name":"spawn_subagent"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-resume","name":"spawn_subagent",
                "arguments":{
                    "prompt":"continue the review",
                    "resume_from":"a0123456789abcdef0"
                }
            })),
            &mut state,
        )
        .expect("end");
    let start = receiver.recv().await.expect("start event");
    let call = receiver.recv().await.expect("call event");
    assert_eq!(start["params"]["tool"], "SendMessage");
    assert_eq!(call["params"]["tool"], "SendMessage");
    assert_eq!(
        call["params"]["arguments"],
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
    assert!(call["params"]["arguments"].get("resume_from").is_none());
    assert!(call["params"]["arguments"].get("prompt").is_none());
}

#[tokio::test]
async fn spawn_subagent_resume_without_listed_send_message_emits_no_call() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::event_translate_state(&json!({
        "tools":[{"name":"Agent"},{"name":"Bash"}]
    }));
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-drop","name":"spawn_subagent"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-drop","name":"spawn_subagent",
                "arguments":{
                    "prompt":"continue the review",
                    "resume_from":"a0123456789abcdef0"
                }
            })),
            &mut state,
        )
        .expect("end");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "unlisted SendMessage continue must not emit item/tool/start or item/tool/call"
    );
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"done",
                "reason":"toolUse",
                "terminal":{"state":"complete","output":"tool_use"},
                "message":{"usage":{}}
            })),
            &mut state,
        )
        .expect("done");
    let _usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_eq!(done["params"]["turn"]["providerStopReason"], "end_turn");
    assert_eq!(
        done["params"]["turn"]["terminal"]["state"],
        "recoverable_error"
    );
}

#[tokio::test]
async fn agent_resume_from_emits_send_message() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::event_translate_state(&json!({
        "tools":[{"name":"Agent"},{"name":"SendMessage"}]
    }));
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-agent","name":"Agent"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-agent","name":"Agent",
                "arguments":{
                    "prompt":"continue the review",
                    "resume":"a0123456789abcdef0"
                }
            })),
            &mut state,
        )
        .expect("end");
    let start = receiver.recv().await.expect("start event");
    let call = receiver.recv().await.expect("call event");
    assert_eq!(start["params"]["tool"], "SendMessage");
    assert_eq!(call["params"]["tool"], "SendMessage");
    assert_eq!(
        call["params"]["arguments"],
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[tokio::test]
async fn spawn_subagent_resume_prefers_buffered_prompt_over_partial_event_args() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::event_translate_state(&json!({
        "tools":[{"name":"SendMessage"}]
    }));
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-buf","name":"spawn_subagent"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,
                "delta":"{\"prompt\":\"continue the review\",\"resume_from\":\"a0123456789abcdef0\"}"
            })),
            &mut state,
        )
        .expect("delta");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "SendMessage must wait for complete terminal arguments"
    );
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-buf","name":"spawn_subagent",
                "arguments":{"resume_from":"a0123456789abcdef0"}
            })),
            &mut state,
        )
        .expect("end");
    let start = receiver.recv().await.expect("start event");
    let call = receiver.recv().await.expect("call event");
    assert_eq!(start["params"]["tool"], "SendMessage");
    assert_eq!(call["params"]["tool"], "SendMessage");
    assert_eq!(
        call["params"]["arguments"],
        json!({"to":"a0123456789abcdef0","message":"continue the review"})
    );
}

#[tokio::test]
async fn done_recovers_unclosed_send_message_from_authoritative_message() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::event_translate_state(&json!({
        "tools":[{"name":"SendMessage"}]
    }));
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-terminal",
                "name":"SendMessage"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,
                "delta":"{\"to\":\"main\",\"message\":\"validation passed\"}"
            })),
            &mut state,
        )
        .expect("delta");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv())
            .await
            .is_err(),
        "SendMessage must not expose a partial streaming card"
    );
    assert!(
        gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"done",
                    "reason":"toolUse",
                    "terminal":{"state":"complete","output":"tool_use"},
                    "message":{
                        "content":[{
                            "type":"toolCall","id":"call-terminal","name":"SendMessage",
                            "arguments":{"to":"main","message":"validation passed"}
                        }],
                        "usage":{}
                    }
                })),
                &mut state,
            )
            .expect("done")
    );
    let start = receiver.recv().await.expect("recovered start event");
    let call = receiver.recv().await.expect("recovered call event");
    let _usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_eq!(start["params"]["tool"], "SendMessage");
    assert_eq!(call["params"]["tool"], "SendMessage");
    assert_eq!(
        call["params"]["arguments"],
        json!({"to":"main","message":"validation passed"})
    );
    assert_eq!(done["params"]["turn"]["providerStopReason"], "tool_use");
    assert_eq!(done["params"]["turn"]["terminal"]["state"], "complete");
}

#[tokio::test]
async fn empty_bash_does_not_emit_tool_start() {
    let gateway = gateway();
    let receiver = gateway.events.subscribe("request");
    let mut state = super::EventTranslateState::default();
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_start","index":0,"toolCallId":"call-bash","name":"Bash"
            })),
            &mut state,
        )
        .expect("start");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_delta","index":0,"delta":"{}"
            })),
            &mut state,
        )
        .expect("delta");
    gateway
        .handle_event(
            "thread",
            "request",
            &event(json!({
                "type":"toolcall_end","index":0,"toolCallId":"call-bash","name":"Bash",
                "arguments":{}
            })),
            &mut state,
        )
        .expect("end");
    match tokio::time::timeout(std::time::Duration::from_millis(20), receiver.recv()).await {
        Err(_) => {}
        Ok(None) => {}
        Ok(Some(frame)) => panic!("empty Bash must not emit {frame}"),
    }
    assert!(
        gateway
            .handle_event(
                "thread",
                "request",
                &event(json!({
                    "type":"done",
                    "reason":"toolUse",
                    "terminal":{"state":"complete","output":"tool_use"},
                    "message":{"usage":{}}
                })),
                &mut state,
            )
            .expect("done")
    );
    let _usage = receiver.recv().await.expect("usage event");
    let done = receiver.recv().await.expect("done event");
    assert_eq!(done["params"]["turn"]["providerStopReason"], "end_turn");
    assert_eq!(
        done["params"]["turn"]["terminal"],
        json!({
            "state":"recoverable_error",
            "output":"none",
            "code":"tool_use_without_forwarded_call"
        })
    );
}
