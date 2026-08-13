// Coverage gates measure production code; test implementations are excluded.
#![cfg_attr(coverage_nightly, coverage(off))]

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    process::Command,
    sync::{Arc, Barrier, Mutex},
    thread,
    time::{Duration, Instant},
};

use super::records::launch_records;
use super::records_scope::{latest_user_text, scope_similarity};
use super::store::{CACHE_VERSION, ClaimRequest, Store, StoredStates, current_pid, unix_seconds};
use super::*;

fn request(session: &str, messages: Vec<Value>) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: Value::String("stable system".to_owned()),
        messages,
        // These tests exercise explicit Agent Teams reuse. Ordinary
        // Agent/Task sessions are covered separately and must not receive
        // mailbox guidance.
        tools: vec![
            json!({"name":"Agent"}),
            json!({"name":"SendMessage"}),
            json!({"name":"TeamSendMessage"}),
        ],
        stream: false,
        output_config: Value::Null,
        metadata: json!({
            "_claudex_transport_identity":{"session_id":session}
        }),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn launch(tool_use_id: &str, recipient: &str) -> Value {
    json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":tool_use_id,
            "content":[{"type":"text","text":format!("Async agent launched successfully.\\nagentId: {recipient}")}]
        }]
    })
}

fn launch_with_context(tool_use_id: &str, recipient: &str) -> Vec<Value> {
    vec![
        json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":tool_use_id,
                "name":"Agent",
                "input":{
                    "prompt":"Audit Rust adapter tests and preserve the active worker",
                    "claudex_model":"worker-model"
                }
            }]
        }),
        launch(tool_use_id, recipient),
        json!({
            "role":"user",
            "content":"<task-id>worker-a</task-id><status>completed</status>"
        }),
    ]
}

fn launch_with_scope(tool_use_id: &str, recipient: &str, scope: &str, model: &str) -> Vec<Value> {
    vec![
        json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":tool_use_id,
                "name":"Agent",
                "input":{"prompt":scope,"claudex_model":model}
            }]
        }),
        launch(tool_use_id, recipient),
    ]
}

#[test]
fn live_agent_task_ids_keep_active_claude_code_ids_only() {
    let mut messages = launch_with_scope(
        "tool-live",
        "a4496564387a2561f",
        "Implement AzooKey pruning fix",
        "worker-model",
    );
    messages.push(launch("tool-name", "worker-a"));
    messages.push(launch("tool-done", "a906c77ad60469b0a"));
    messages.push(json!({
        "role":"user",
        "content":"<task-id>a906c77ad60469b0a</task-id><status>completed</status>"
    }));
    assert_eq!(
        live_agent_task_ids(&messages),
        vec!["a4496564387a2561f".to_owned()]
    );
}

#[test]
fn records_recipients_and_persists_across_registry_restart() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let registry = SubagentReuseRegistry::with_store(path.clone());
    let mut first = request("session-a", vec![launch("tool-a", "worker-a")]);
    registry.observe_and_restore(&mut first);
    assert_eq!(
        registry.state_for("session-a"),
        Some(vec!["worker-a".to_owned()])
    );

    let restored = SubagentReuseRegistry::with_store(path);
    let mut resumed = request(
        "session-a",
        vec![json!({"role":"user","content":"compact summary"})],
    );
    restored.observe_and_restore(&mut resumed);
    assert!(resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
    assert!(resumed.system.to_string().contains("worker-a"));
}

#[test]
fn unchanged_transcript_skips_redundant_persistence() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let registry = SubagentReuseRegistry::with_store(path.clone());
    let messages = vec![launch("tool-a", "worker-a")];

    registry.observe_and_restore(&mut request("session-a", messages.clone()));
    let inode =
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).expect("persisted registry"));

    registry.observe_and_restore(&mut request("session-a", messages));
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).expect("persisted registry")),
        inode,
        "replaying the same transcript must not fsync the cache again"
    );
}

#[test]
fn duplicate_history_does_not_inflate_cumulative_spawn_count() {
    let registry = SubagentReuseRegistry::default();
    let mut request = request(
        "session-a",
        vec![launch("tool-a", "worker-a"), launch("tool-a", "worker-a")],
    );
    registry.observe_and_restore(&mut request);
    assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
}

#[test]
fn semantic_duplicate_scope_does_not_create_a_second_worker() {
    let registry = SubagentReuseRegistry::default();
    let mut messages = launch_with_scope(
        "tool-a",
        "worker-a",
        "Audit the Rust adapter tests",
        "worker-model",
    );
    messages.extend(launch_with_scope(
        "tool-b",
        "worker-b",
        "  audit   the rust adapter tests  ",
        "worker-model",
    ));
    let mut request = request("session-a", messages);
    registry.observe_and_restore(&mut request);
    assert_eq!(
        registry.state_for("session-a"),
        Some(vec!["worker-a".to_owned()])
    );
}

#[test]
fn terminal_worker_can_be_relaunched_for_a_new_attempt() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut second_messages = launch_with_scope(
        "tool-b",
        "worker-b",
        "Audit the Rust adapter tests",
        "worker-model",
    );
    second_messages.insert(
        0,
        json!({"role":"user","content":"<task-id>worker-a</task-id><status>completed</status>"}),
    );
    let mut second = request("session-a", second_messages);
    registry.observe_and_restore(&mut second);
    assert_eq!(
        registry.state_for("session-a"),
        Some(vec!["worker-a".to_owned(), "worker-b".to_owned()])
    );
}

#[test]
fn active_worker_launch_with_same_scope_does_not_create_duplicate() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut second = request(
        "session-a",
        launch_with_scope(
            "tool-b",
            "worker-b",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut second);
    assert_eq!(
        registry.state_for("session-a"),
        Some(vec!["worker-a".to_owned()])
    );
}

#[test]
fn queued_send_message_status_is_preserved_for_reuse() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut resumed = request(
        "session-a",
        vec![json!({
            "role":"user",
            "content":[{"type":"tool_result","tool_use_id":"send-a","content":"Agent \"worker-a\" had no active task; resumed from transcript in the background with your message."}]
        })],
    );
    registry.observe_and_restore(&mut resumed);
    assert!(resumed.system.to_string().contains("worker-a"));
    assert!(
        !resumed.system.to_string().contains("message_queued"),
        "reusable status must stay out of system guidance for prompt-cache stability"
    );
    assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
}

#[test]
fn reuse_guidance_keeps_stable_recipient_order_across_user_turns() {
    let registry = SubagentReuseRegistry::default();
    let mut initial = request(
        "session-a",
        [
            launch_with_scope("tool-a", "worker-css", "Review CSS layout", "css-model"),
            launch_with_scope(
                "tool-b",
                "worker-rust",
                "Audit Rust adapter tests",
                "rust-model",
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    registry.observe_and_restore(&mut initial);
    let mut rust_turn = request(
        "session-a",
        vec![json!({"role":"user","content":"continue Rust adapter tests"})],
    );
    registry.observe_and_restore(&mut rust_turn);
    let mut css_turn = request(
        "session-a",
        vec![json!({"role":"user","content":"continue CSS layout review"})],
    );
    registry.observe_and_restore(&mut css_turn);
    let rust_guidance = rust_turn.system.to_string();
    let css_guidance = css_turn.system.to_string();
    assert_eq!(
        rust_guidance, css_guidance,
        "system reuse guidance must stay byte-stable so prompt-cache signatures do not churn"
    );
    assert!(
        rust_guidance.find("worker-css").expect("css worker")
            < rust_guidance.find("worker-rust").expect("rust worker"),
        "recipients stay lexicographically stable regardless of the latest user task"
    );
}

#[test]
fn restores_scope_model_and_status_for_dynamic_recipient_assignment() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let registry = SubagentReuseRegistry::with_store(path.clone());
    let mut first = request("session-a", launch_with_context("tool-a", "worker-a"));
    registry.observe_and_restore(&mut first);

    let restored = SubagentReuseRegistry::with_store(path);
    let mut resumed = request(
        "session-a",
        vec![json!({"role":"user","content":"continue Rust adapter tests"})],
    );
    restored.observe_and_restore(&mut resumed);
    let guidance = resumed.system.to_string();
    assert!(guidance.contains("worker-a"));
    assert!(guidance.contains("Audit Rust adapter tests"));
    assert!(guidance.contains("worker-model"));
    assert!(
        !guidance.contains("completed"),
        "status churn must not appear in reuse guidance"
    );
}

#[test]
fn restores_reuse_guidance_when_transcript_still_lists_launches() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request("session-a", launch_with_context("tool-a", "worker-a"));
    registry.observe_and_restore(&mut first);
    // Full transcript still has the launch result (observed non-empty), but
    // system was rebuilt without the marker.
    let mut resumed = request("session-a", launch_with_context("tool-a", "worker-a"));
    registry.observe_and_restore(&mut resumed);
    assert!(
        resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER),
        "guidance must restore even when launch results remain in messages"
    );
    assert!(resumed.system.to_string().contains("worker-a"));
}

#[test]
fn failed_auto_resume_retires_recipient_so_rewrite_stops() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut failed_resume = request(
        "session-a",
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-resume",
                    "name":"Agent",
                    "input":{
                        "prompt":"Audit the Rust adapter tests",
                        "claudex_model":"worker-model",
                        "resume":"worker-a"
                    }
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"tool-resume",
                    "is_error":true,
                    "content":"No agent found with ID worker-a"
                }]
            }),
        ],
    );
    registry.observe_and_restore(&mut failed_resume);
    let mut follow = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut follow),
        None,
        "failed resume target must not be reinjected forever"
    );
    assert!(follow.get("resume").is_none());
}

#[test]
fn successful_resume_without_spawn_phrase_keeps_recipient_reusable() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut resumed = request(
        "session-a",
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-resume",
                    "name":"Agent",
                    "input":{
                        "prompt":"Audit the Rust adapter tests",
                        "claudex_model":"worker-model",
                        "resume":"worker-a"
                    }
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"tool-resume",
                    "content":"Resumed agent worker-a; work continues."
                }]
            }),
        ],
    );
    registry.observe_and_restore(&mut resumed);
    let mut follow = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut follow),
        Some("worker-a".to_owned()),
        "successful resume prose must not retire the recipient"
    );
    assert_eq!(
        follow.get("resume").and_then(Value::as_str),
        Some("worker-a")
    );
}

#[test]
fn launch_tools_are_hidden_only_after_the_session_budget_is_reached() {
    let mut below = request("session-a", Vec::new());
    set_limit_metadata(&mut below, false);
    assert!(should_expose_launch_tools(&below));
    let mut reached = request("session-a", Vec::new());
    set_limit_metadata(&mut reached, true);
    assert!(!should_expose_launch_tools(&reached));
    assert_eq!(DEFAULT_MAX_SUBAGENTS_PER_SESSION, 1_024);
    let mut null_metadata = request("session-a", Vec::new());
    null_metadata.metadata = Value::Null;
    set_limit_metadata(&mut null_metadata, true);
    assert!(null_metadata.metadata.is_object());
}

#[test]
fn empty_ids_and_corrupt_store_do_not_rewrite_or_occupy_scope() {
    let registry = SubagentReuseRegistry::default();
    let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(registry.rewrite_launch_input("", &mut arguments), None);
    assert!(!registry.scope_is_occupied("", &arguments));
    assert!(!registry.scope_is_occupied("session-a", &json!({})));
    registry.note_inflight_launch("", &arguments, "tool-a");
    registry.note_inflight_launch("session-a", &arguments, "");
    registry.note_inflight_launch("session-a", &json!({}), "tool-a");
    assert!(!registry.scope_is_occupied("session-a", &arguments));

    let root = tempfile::tempdir().expect("reuse store fixture");
    let corrupt = root.path().join("corrupt.json");
    fs::write(&corrupt, "{not json").expect("corrupt cache");
    let _ignored = SubagentReuseRegistry::with_store(corrupt);

    let incompatible = root.path().join("old.json");
    fs::write(&incompatible, r#"{"version":0,"sessions":{}}"#).expect("old cache");
    let _ignored = SubagentReuseRegistry::with_store(incompatible);

    let not_a_dir = root.path().join("not-a-dir");
    fs::write(&not_a_dir, "x").expect("file where directory should be");
    let failing = SubagentReuseRegistry::with_store(not_a_dir.join("cache.json"));
    let mut first = request("session-a", launch_with_context("tool-a", "worker-a"));
    failing.observe_and_restore(&mut first);
}

#[test]
fn ordinary_agent_session_does_not_restore_mailbox_guidance() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request("ordinary-session", vec![launch("tool-a", "worker-a")]);
    first.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
    registry.observe_and_restore(&mut first);

    let mut resumed = request(
        "ordinary-session",
        vec![json!({"role":"user","content":"continue"})],
    );
    resumed.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
    registry.observe_and_restore(&mut resumed);
    let guidance = resumed.system.to_string();
    assert!(guidance.contains(REUSE_GUIDANCE_MARKER));
    assert!(guidance.contains("worker-a"));
    assert!(guidance.contains("resume"));
    assert!(!guidance.contains("TeamSendMessage"));
}

fn launch_arguments(prompt: &str, model: &str) -> Value {
    json!({
        "prompt": prompt,
        "claudex_model": model,
        "subagent_type": "claudex-worker",
        "run_in_background": true
    })
}

#[test]
fn reuse_keys_off_user_id_json_session_when_transport_header_is_missing() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request("ignored", vec![launch("tool-a", "worker-a")]);
    first.metadata = json!({"user_id": r#"{"session_id":"from-user"}"#});
    registry.observe_and_restore(&mut first);
    assert_eq!(
        registry.state_for("from-user"),
        Some(vec!["worker-a".to_owned()])
    );
    assert_eq!(registry.state_for("ignored"), None);
}

#[test]
fn concurrent_claude_sessions_do_not_reuse_each_others_workers() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);

    let mut peer = request(
        "session-b",
        vec![json!({"role":"user","content":"independent tui"})],
    );
    registry.observe_and_restore(&mut peer);
    assert!(!peer.system.to_string().contains("worker-a"));
    assert!(
        registry
            .state_for("session-b")
            .unwrap_or_default()
            .is_empty()
    );

    let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-b", &mut arguments),
        None,
        "another claudex TUI must not resume this session's SubAgent"
    );
    assert!(arguments.get("resume").is_none());
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        Some("worker-a".to_owned())
    );
}

#[test]
fn same_scope_active_launch_is_rewritten_to_resume_instead_of_a_new_spawn() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        Some("worker-a".to_owned())
    );
    assert_eq!(arguments["resume"], "worker-a");
    assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
}

#[test]
fn completed_same_scope_worker_is_revived_with_resume_instead_of_a_new_spawn() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut completed = request(
        "session-a",
        vec![
            json!({"role":"user","content":"<task-id>worker-a</task-id><status>completed</status>"}),
        ],
    );
    registry.observe_and_restore(&mut completed);
    let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        Some("worker-a".to_owned())
    );
    assert_eq!(arguments["resume"], "worker-a");
}

#[test]
fn failed_or_stopped_worker_is_not_rewritten_to_resume() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    for status in ["failed", "cancelled", "stopped"] {
        let mut terminal = request(
            "session-a",
            vec![
                json!({"role":"user","content":format!("<task-id>worker-a</task-id><status>{status}</status>")}),
            ],
        );
        registry.observe_and_restore(&mut terminal);
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None,
            "{status} workers must stay launchable as a fresh spawn"
        );
        assert!(arguments.get("resume").is_none());
    }
}

#[test]
fn independent_scope_still_launches_a_new_worker() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = launch_arguments("Review CSS layout", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        None
    );
    assert!(arguments.get("resume").is_none());
}

#[test]
fn explicit_resume_is_left_alone() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = json!({
        "prompt":"Audit the Rust adapter tests",
        "claudex_model":"worker-model",
        "resume":"already-chosen"
    });
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        None
    );
    assert_eq!(arguments["resume"], "already-chosen");
}

#[test]
fn same_scope_different_model_stays_independent_fanout() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = launch_arguments("Audit the Rust adapter tests", "other-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        None,
        "distinct claudex_model must spawn a peer, not resume the other model"
    );
    assert!(arguments.get("resume").is_none());
}

#[test]
fn description_same_scope_same_model_prefers_existing_worker() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-a",
                    "name":"Agent",
                    "input":{
                        "description":"Reproduce azookey conversion bug",
                        "prompt":"Use gpt to map the conversion pipeline.",
                        "claudex_model":"gpt-test"
                    }
                }]
            }),
            launch("tool-a", "worker-a"),
        ],
    );
    registry.observe_and_restore(&mut first);
    let mut peer = json!({
        "description":"Reproduce azookey conversion bug",
        "prompt":"Use command code to map the conversion pipeline.",
        "claudex_model":"command-code"
    });
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut peer),
        None,
        "description match alone must not cross claudex_model boundaries"
    );
    let mut same_model = json!({
        "description":"Reproduce azookey conversion bug",
        "prompt":"Continue mapping the conversion pipeline.",
        "claudex_model":"gpt-test"
    });
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut same_model),
        Some("worker-a".to_owned())
    );
}

#[test]
fn inflight_placeholder_occupies_scope_before_tool_result() {
    let registry = SubagentReuseRegistry::default();
    let mut arguments = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Start with Vibrato boundaries.",
        "claudex_model":"gpt-test"
    });
    registry.note_inflight_launch("session-a", &arguments, "tool-pending");
    assert!(registry.scope_is_occupied("session-a", &arguments));
    assert!(
        registry
            .rewrite_launch_input("session-a", &mut arguments)
            .is_none()
    );
    assert_eq!(registry.state_for("session-a"), Some(Vec::<String>::new()));
}

#[test]
fn store_backed_claims_occupy_scope_after_memory_is_forgotten() {
    let root = tempfile::tempdir().expect("reuse store");
    let path = root.path().join("subagent-recipients-v1.json");
    let registry = SubagentReuseRegistry::with_store(path);
    let arguments = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Start with Vibrato boundaries.",
        "claudex_model":"gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-pending"));
    registry.forget_memory_for_test();
    assert!(
        registry.scope_is_occupied("session-a", &arguments),
        "persisted claims must occupy the scope without in-memory launches"
    );
}

#[test]
fn parallel_inflight_placeholders_with_empty_recipients_stay_distinct() {
    let registry = SubagentReuseRegistry::default();
    let scope_a = json!({
        "description":"Recover ComfyUI post-reboot state",
        "prompt":"Inventory :8188 and output MP4s.",
        "claudex_model":"gpt-5.6-luna"
    });
    let scope_b = json!({
        "description":"Fix SubAgent progress chrome",
        "prompt":"Paint Thinking tip during raw CoT.",
        "claudex_model":"gpt-5.6-luna"
    });
    registry.note_inflight_launch("session-a", &scope_a, "tool-comfy");
    registry.note_inflight_launch("session-a", &scope_b, "tool-progress");
    assert!(
        registry.scope_is_occupied("session-a", &scope_a),
        "first parallel scope must stay occupied"
    );
    assert!(
        registry.scope_is_occupied("session-a", &scope_b),
        "second parallel scope must not collapse into the first empty-recipient placeholder"
    );
    let states = registry.states.lock().expect("lock");
    let launches = &states.get("session-a").expect("session").launches;
    assert_eq!(
        launches.len(),
        2,
        "empty-recipient inflight notes must not merge across tool_use ids: {launches:?}"
    );
    assert!(
        launches
            .iter()
            .any(|launch| launch.key == "tool-comfy" && launch.status == "pending")
    );
    assert!(
        launches
            .iter()
            .any(|launch| launch.key == "tool-progress" && launch.status == "pending")
    );
}

#[test]
fn same_scope_different_models_are_independent_fanout() {
    let registry = SubagentReuseRegistry::default();
    let gpt = json!({
        "description":"Recover post-reboot state",
        "prompt":"Check ComfyUI and /loop.",
        "claudex_model":"gpt-5.6-luna"
    });
    let cursor = json!({
        "description":"Recover post-reboot state",
        "prompt":"Check ComfyUI and /loop.",
        "claudex_model":"auto"
    });
    let muse = json!({
        "description":"Recover post-reboot state",
        "prompt":"Check ComfyUI and /loop.",
        "claudex_model":"meta/muse-spark-1.2-contributor"
    });
    registry.note_inflight_launch("session-a", &gpt, "tool-gpt");
    registry.note_inflight_launch("session-a", &cursor, "tool-cursor");
    registry.note_inflight_launch("session-a", &muse, "tool-muse");
    assert!(registry.scope_is_occupied("session-a", &gpt));
    assert!(registry.scope_is_occupied("session-a", &cursor));
    assert!(registry.scope_is_occupied("session-a", &muse));
    let states = registry.states.lock().expect("lock");
    let launches = &states.get("session-a").expect("session").launches;
    assert_eq!(
        launches.len(),
        3,
        "same description with distinct claudex_model must stay parallel: {launches:?}"
    );
}

#[test]
fn failed_launch_result_releases_inflight_scope() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Start with Vibrato boundaries.",
        "claudex_model":"gpt-test"
    });
    registry.note_inflight_launch("session-a", &arguments, "tool-failed");
    let mut request = request(
        "session-a",
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-failed",
                    "name":"Agent",
                    "input":arguments
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"tool-failed",
                    "is_error":true,
                    "content":"provider launch failed"
                }]
            }),
        ],
    );
    registry.observe_and_restore(&mut request);
    assert!(!registry.scope_is_occupied("session-a", &arguments));
}

#[test]
fn unique_fuzzy_scope_overlap_does_not_rewrite_independent_fanout() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = launch_arguments("continue Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        None,
        "fuzzy follow-ups stay as new launches; only exact same-scope launches resume"
    );
}

#[test]
fn three_independent_pathspec_scopes_are_not_rewritten_onto_one_worker() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "R2 catalog sync full perf",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    for prompt in [
        "Commit only catalog pathspec A",
        "Commit only queue pathspec B",
        "Commit only worker pathspec C",
    ] {
        let mut arguments = launch_arguments(prompt, "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None,
            "{prompt} must stay an independent launch"
        );
    }
}

#[test]
fn ambiguous_similar_workers_are_not_guessed() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        [
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit Rust adapter tests",
                "worker-model",
            ),
            launch_with_scope(
                "tool-b",
                "worker-b",
                "Review Rust error handling",
                "worker-model",
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    registry.observe_and_restore(&mut first);
    let mut arguments = launch_arguments("continue Rust work", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut arguments),
        None
    );
}

#[test]
fn reuse_disabled_does_not_rewrite_or_restore_guidance() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore_for_test(&mut first, false);
    let mut resumed = request(
        "session-a",
        vec![json!({"role":"user","content":"continue Rust adapter tests"})],
    );
    registry.observe_and_restore_for_test(&mut resumed, false);
    assert!(!resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
    let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
    assert_eq!(
        registry.rewrite_launch_input_for_test("session-a", &mut arguments, false),
        None
    );
}

#[test]
fn resume_of_completed_worker_does_not_increment_spawn_count() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request(
        "session-a",
        launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ),
    );
    registry.observe_and_restore(&mut first);
    let mut messages = vec![json!({
        "role":"user",
        "content":"<task-id>worker-a</task-id><status>completed</status>"
    })];
    messages.extend(launch_with_scope(
        "tool-c",
        "worker-a",
        "Audit the Rust adapter tests",
        "worker-model",
    ));
    let mut resumed = request("session-a", messages);
    registry.observe_and_restore(&mut resumed);
    assert_eq!(
        registry.state_for("session-a"),
        Some(vec!["worker-a".to_owned()])
    );
    assert_eq!(
        registry.status_for("session-a", "worker-a").as_deref(),
        Some("active")
    );
}

#[test]
fn named_agent_input_alone_does_not_enable_agent_teams_mailbox() {
    let mut ordinary = request(
        "ordinary-named-worker",
        vec![json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":"agent-call",
                "name":"Agent",
                "input":{"name":"research-worker","run_in_background":true}
            }]
        })],
    );
    ordinary.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
    assert!(!agent_teams_enabled(&ordinary));
    let (tools, _, _) = crate::anthropic::session::tool_configuration(&ordinary, None, None);
    assert!(
        tools
            .iter()
            .all(|tool| { tool.get("name").and_then(Value::as_str) != Some("cc_SendMessage_1") })
    );
}

#[test]
fn generic_agent_teams_documentation_does_not_enable_mailbox_transport() {
    let mut ordinary = request(
        "ordinary-documentation",
        vec![json!({
            "role":"user",
            "content":"The Agent Teams documentation is present, but no team was requested."
        })],
    );
    ordinary.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
    assert!(!agent_teams_enabled(&ordinary));
}

#[test]
fn explicit_agent_teams_session_restores_mailbox_guidance() {
    let registry = SubagentReuseRegistry::default();
    let mut first = request("team-session", vec![launch("tool-a", "worker-a")]);
    registry.observe_and_restore(&mut first);

    let mut resumed = request(
        "team-session",
        vec![json!({"role":"user","content":"continue"})],
    );
    registry.observe_and_restore(&mut resumed);
    assert!(resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
    assert!(agent_teams_enabled(&resumed));
}

#[test]
fn only_native_launch_results_are_recorded() {
    let request = request(
        "session-a",
        vec![json!({
            "role":"user",
            "content":[{"type":"tool_result","tool_use_id":"read","content":"agentId: not-a-launch"}]
        })],
    );
    assert!(launch_records(&request.messages).is_empty());
}

#[test]
fn launch_records_cover_empty_scope_and_background_spawn_text() {
    assert!(summarize_scope(&json!({})).is_empty());
    assert!(
        summarize_scope(&json!({"prompt":"\nclaudex_hidden\n<claudex-note>skip</claudex-note>\n"}))
            .is_empty()
    );
    assert!(!already_has_resume(&json!({})));
    assert!(!already_has_resume(&json!({"resume":""})));
    assert!(already_has_resume(&json!({"resume":"session-1"})));
    assert!(find_reusable_launch(&[], &json!({})).is_none());
    assert!(!scope_is_occupied(&[], "", None));

    let records = launch_records(&[
        json!({"role":"assistant"}),
        json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":"call-read",
                "name":"Read",
                "input":{"path":"CLAUDE.md"}
            }]
        }),
        json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"call-a",
                "content":[{"type":"text","text":"teammate_spawned agentId: worker-b"}]
            }]
        }),
        json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"call-c",
                "content":[{"type":"text","text":"working in the background\nagent_id: worker-c"}]
            }]
        }),
    ]);
    assert!(
        records.iter().any(|record| record.recipient == "worker-b"),
        "{records:?}"
    );
    assert!(
        records.iter().any(|record| record.recipient == "worker-c"),
        "{records:?}"
    );
}

#[test]
fn launch_records_accept_agent_and_task_id_json_fields() {
    let json_id = launch_records(&[json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"call-json",
            "agentId":"worker-json",
            "content":[{"type":"text","text":"Async agent launched successfully."}]
        }]
    })]);
    assert!(
        json_id
            .iter()
            .any(|record| record.recipient == "worker-json"),
        "{json_id:?}"
    );

    let task_json = launch_records(&[json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"call-task",
            "taskId":"worker-task",
            "content":[{"type":"text","text":"Async agent launched successfully."}]
        }]
    })]);
    assert!(
        task_json
            .iter()
            .any(|record| record.recipient == "worker-task"),
        "taskId JSON field must seed resume recipient: {task_json:?}"
    );

    let task_text = launch_records(&[json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"call-task-text",
            "content":[{"type":"text","text":"Async agent launched successfully.\ntaskId: worker-task-text"}]
        }]
    })]);
    assert!(
        task_text
            .iter()
            .any(|record| record.recipient == "worker-task-text"),
        "taskId text marker must seed resume recipient: {task_text:?}"
    );
}

#[test]
fn scope_similarity_prioritizes_the_matching_worker() {
    assert!(
        scope_similarity("audit Rust adapter tests", "continue Rust tests")
            > scope_similarity("review CSS layout", "continue Rust tests")
    );
}

#[test]
fn concurrent_persistence_does_not_race_the_atomic_replace() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Arc::new(Store::new(path.clone()));
    let barrier = Arc::new(Barrier::new(16));
    let threads = (0..16)
        .map(|index| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let mut states = HashMap::new();
                states.insert(format!("session-{index}"), SessionState::default());
                store.save(states)
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread
            .join()
            .expect("persistence thread")
            .expect("serialized persistence");
    }
    let bytes = std::fs::read(path).expect("persisted registry");
    serde_json::from_slice::<StoredStates>(&bytes).expect("valid registry JSON");
}

#[test]
fn literal_v1_cache_migrates_to_v2_without_losing_sessions() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let legacy = json!({
        "version": 1,
        "sessions": {
            "session-a": {
                "launches": [{
                    "key": "tool-a",
                    "recipient": "worker-a",
                    "scope": "Audit Rust",
                    "model": "worker-model",
                    "status": "active"
                }]
            }
        }
    });
    fs::write(&path, serde_json::to_vec(&legacy).expect("legacy JSON"))
        .expect("write legacy cache");

    let store = Store::new(path.clone());
    assert_eq!(
        store.load_snapshot().sessions["session-a"].launches[0].recipient,
        "worker-a"
    );
    store
        .save(store.load_snapshot().sessions)
        .expect("rewrite migrated cache");
    let migrated = serde_json::from_slice::<StoredStates>(&fs::read(path).expect("v2 cache"))
        .expect("v2 JSON");
    assert_eq!(migrated.version, CACHE_VERSION);
    assert_eq!(migrated.sessions["session-a"].launches[0].key, "tool-a");
}

#[test]
fn stale_session_delta_cannot_resurrect_a_deleted_session() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let mut initial = HashMap::new();
    initial.insert(
        "session-a".to_owned(),
        SessionState {
            launches: vec![LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                status: "active".to_owned(),
            }],
        },
    );
    store.save(initial.clone()).expect("initial snapshot");
    let base_revision = store
        .load_snapshot()
        .session_revisions
        .get("session-a")
        .copied()
        .expect("session revision");
    assert!(
        store
            .delete_session("session-a", base_revision)
            .expect("delete session")
    );
    assert!(
        !store
            .save_session_delta("session-a", initial["session-a"].clone(), base_revision)
            .expect("stale delta")
    );
    let loaded = store.load_snapshot();
    assert!(!loaded.sessions.contains_key("session-a"));
    assert!(loaded.tombstones.contains_key("session-a"));
}

#[test]
fn claims_reap_dead_and_expired_leases_and_fence_stale_release() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let now = unix_seconds();
    let scope = "Audit Rust";
    let dead = store
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: scope.to_owned(),
                model: Some("worker-model".to_owned()),
                owner: "dead-owner".to_owned(),
                pid: 0,
                tool_use_id: "dead-tool".to_owned(),
                expires_unix_seconds: now + 60,
            },
            now,
        )
        .expect("dead claim acquisition")
        .expect("dead claim");
    assert!(
        !store
            .claims_occupy("session-a", scope, Some("worker-model"), now)
            .expect("dead claim reap")
    );
    assert_eq!(dead.pid, 0);

    let live = store
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: scope.to_owned(),
                model: Some("worker-model".to_owned()),
                owner: "owner-a".to_owned(),
                pid: current_pid(),
                tool_use_id: "live-tool".to_owned(),
                expires_unix_seconds: now + 60,
            },
            now,
        )
        .expect("live claim acquisition")
        .expect("live claim");
    let mut stale = live.clone();
    stale.created_revision = stale.created_revision.saturating_sub(1);
    assert!(!store.release_claim(&stale, now).expect("stale release"));
    let mut foreign = live.clone();
    foreign.owner = "owner-b".to_owned();
    assert!(!store.release_claim(&foreign, now).expect("foreign release"));
    assert!(
        store
            .claims_occupy("session-a", scope, Some("worker-model"), now)
            .expect("live claim occupancy")
    );
    assert!(store.release_claim(&live, now).expect("owner release"));
    assert!(
        !store
            .claims_occupy("session-a", scope, Some("worker-model"), now)
            .expect("released claim occupancy")
    );

    let expired = store
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: scope.to_owned(),
                model: Some("worker-model".to_owned()),
                owner: "expired-owner".to_owned(),
                pid: current_pid(),
                tool_use_id: "expired-tool".to_owned(),
                expires_unix_seconds: now.saturating_sub(1),
            },
            now,
        )
        .expect("expired claim acquisition")
        .expect("expired claim");
    assert!(
        !store
            .claims_occupy("session-a", scope, Some("worker-model"), now)
            .expect("expired claim reap")
    );
    assert_eq!(expired.expires_unix_seconds, now.saturating_sub(1));
}

#[test]
fn barrier_admission_allows_only_one_same_scope_claim() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = Arc::new(root.path().join("reuse.json"));
    let barrier = Arc::new(Barrier::new(12));
    // Spawn every contender before joining so the barrier can release together.
    let mut threads = Vec::with_capacity(12);
    for index in 0..12 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let store = Store::new((*path).clone());
            barrier.wait();
            store
                .acquire_claim(
                    ClaimRequest {
                        session_id: "session-a".to_owned(),
                        scope: "same scope".to_owned(),
                        model: Some("worker-model".to_owned()),
                        owner: format!("owner-{index}"),
                        pid: current_pid(),
                        tool_use_id: format!("tool-{index}"),
                        expires_unix_seconds: unix_seconds() + 60,
                    },
                    unix_seconds(),
                )
                .expect("claim admission")
                .is_some()
        }));
    }
    let admitted = threads
        .into_iter()
        .map(|thread| thread.join().expect("admission thread"))
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(admitted, 1, "one lease must win same-scope admission");
}

#[test]
fn subprocess_barrier_admission_allows_only_one_same_scope_claim() {
    const WORKERS: usize = 6;
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let cache = root.path().join("reuse.json");
    let child_dir = root.path().join("children");
    fs::create_dir(&child_dir).expect("child barrier directory");
    let executable = std::env::current_exe().expect("reuse test executable");
    let mut children = Vec::new();
    for index in 0..WORKERS {
        let child = Command::new(&executable)
            .args([
                "--exact",
                "anthropic::subagent_reuse::tests::subprocess_claim_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("CLAUDEX_REUSE_HELPER_DIR", &child_dir)
            .env("CLAUDEX_REUSE_HELPER_INDEX", index.to_string())
            .env("CLAUDEX_REUSE_HELPER_CACHE", &cache)
            .spawn()
            .expect("spawn claim helper");
        children.push(child);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while (0..WORKERS)
        .filter(|index| child_dir.join(format!("ready-{index}")).exists())
        .count()
        != WORKERS
    {
        assert!(Instant::now() < deadline, "subprocess barrier timed out");
        thread::yield_now();
    }
    fs::write(child_dir.join("release"), b"go").expect("release claim helpers");
    let deadline = Instant::now() + Duration::from_secs(10);
    while (0..WORKERS)
        .filter(|index| child_dir.join(format!("done-{index}")).exists())
        .count()
        != WORKERS
    {
        assert!(
            Instant::now() < deadline,
            "subprocess completion barrier timed out"
        );
        thread::yield_now();
    }
    let winners = (0..WORKERS)
        .filter(|index| child_dir.join(format!("winner-{index}")).exists())
        .count();
    fs::write(child_dir.join("finish"), b"done").expect("finish claim helpers");
    for mut child in children {
        assert!(child.wait().expect("claim helper status").success());
    }
    assert_eq!(winners, 1, "one subprocess must win same-scope admission");
}

#[test]
#[ignore]
fn subprocess_claim_helper() {
    let Some(directory) = std::env::var_os("CLAUDEX_REUSE_HELPER_DIR") else {
        return;
    };
    let index = std::env::var("CLAUDEX_REUSE_HELPER_INDEX").expect("helper index");
    let directory = std::path::PathBuf::from(directory);
    let cache = std::env::var_os("CLAUDEX_REUSE_HELPER_CACHE").expect("helper cache");
    fs::write(directory.join(format!("ready-{index}")), b"ready").expect("ready barrier");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("release").exists() {
        assert!(Instant::now() < deadline, "helper barrier timed out");
        thread::yield_now();
    }
    let now = unix_seconds();
    let admitted = Store::new(std::path::PathBuf::from(cache))
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: "same scope".to_owned(),
                model: Some("worker-model".to_owned()),
                owner: format!("subprocess-owner-{index}"),
                pid: current_pid(),
                tool_use_id: format!("subprocess-tool-{index}"),
                expires_unix_seconds: now + 60,
            },
            now,
        )
        .expect("subprocess claim admission")
        .is_some();
    if admitted {
        fs::write(directory.join(format!("winner-{index}")), b"winner").expect("winner marker");
    }
    fs::write(directory.join(format!("done-{index}")), b"done").expect("done barrier");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("finish").exists() {
        assert!(
            Instant::now() < deadline,
            "helper completion barrier timed out"
        );
        thread::yield_now();
    }
}

#[test]
fn find_reusable_launch_with_empty_scope_returns_none() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-1".to_owned(),
        scope: String::new(),
        model: Some("model-1".to_owned()),
        status: "active".to_owned(),
    }];
    let args = json!({"prompt": "", "claudex_model": "model-1"});
    assert!(find_reusable_launch(&launches, &args).is_none());
}

#[test]
fn find_reusable_launch_requires_nonempty_recipient() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: String::new(),
        scope: "Test scope".to_owned(),
        model: Some("model-1".to_owned()),
        status: "active".to_owned(),
    }];
    let args = json!({"prompt": "Test scope", "claudex_model": "model-1"});
    assert!(find_reusable_launch(&launches, &args).is_none());
}

#[test]
fn already_has_resume_with_nonempty_value() {
    assert!(already_has_resume(&json!({"resume": "worker-a"})));
    assert!(already_has_resume(&json!({"resume": "some-agent"})));
}

#[test]
fn apply_transcript_empty_transcript() {
    let mut launches = vec![];
    apply_transcript(&mut launches, &[]);
    assert!(launches.is_empty());
}

#[test]
fn apply_transcript_with_status_update() {
    let mut launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Test scope".to_owned(),
        model: Some("model-1".to_owned()),
        status: "active".to_owned(),
    }];
    let messages = vec![json!({
        "role": "user",
        "content": "<task-id>worker-a</task-id><status>completed</status>"
    })];
    apply_transcript(&mut launches, &messages);
    assert_eq!(launches[0].status, "completed");
}

#[test]
fn latest_user_text_empty_messages() {
    assert!(latest_user_text(&[]).is_empty());
}

#[test]
fn latest_user_text_with_text_content() {
    let messages = vec![json!({
        "role": "user",
        "content": [{"type": "text", "text": "user message"}]
    })];
    assert_eq!(latest_user_text(&messages), "user message");
}

#[test]
fn latest_user_text_skips_non_user_roles() {
    let messages = vec![
        json!({"role": "assistant", "content": [{"type": "text", "text": "assistant message"}]}),
        json!({"role": "user", "content": [{"type": "text", "text": "user message"}]}),
    ];
    assert_eq!(latest_user_text(&messages), "user message");
}

#[test]
fn scope_similarity_zero_when_no_overlap() {
    let similarity = scope_similarity("audit rust adapter", "review css layout");
    assert_eq!(similarity, 0);
}

#[test]
fn scope_similarity_nonzero_with_matches() {
    let similarity = scope_similarity("audit rust tests", "continue rust work");
    assert!(similarity > 0);
}

#[test]
fn scope_similarity_filters_short_words() {
    let similarity = scope_similarity("a b c test adapter", "x y z test");
    assert!(similarity == 1);
}

#[test]
fn rewrite_launch_input_with_already_has_resume() {
    let registry = SubagentReuseRegistry::default();
    let mut arguments = json!({
        "prompt": "Test",
        "claudex_model": "model-1",
        "resume": "existing-worker"
    });
    assert!(
        registry
            .rewrite_launch_input("session-a", &mut arguments)
            .is_none()
    );
}

#[test]
fn rewrite_launch_input_empty_session_id() {
    let registry = SubagentReuseRegistry::default();
    let mut arguments = json!({"prompt": "Test", "claudex_model": "model-1"});
    assert!(registry.rewrite_launch_input("", &mut arguments).is_none());
}

#[test]
fn find_reusable_launch_prioritizes_active_over_completed() {
    let launches = vec![
        LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-completed".to_owned(),
            scope: "Test scope".to_owned(),
            model: Some("model-1".to_owned()),
            status: "completed".to_owned(),
        },
        LaunchRecord {
            key: "key-2".to_owned(),
            recipient: "worker-active".to_owned(),
            scope: "Test scope".to_owned(),
            model: Some("model-1".to_owned()),
            status: "active".to_owned(),
        },
    ];
    let args = json!({"prompt": "Test scope", "claudex_model": "model-1"});
    let result = find_reusable_launch(&launches, &args);
    assert_eq!(result.map(|r| r.recipient.as_str()), Some("worker-active"));
}

#[test]
fn find_reusable_launch_prefers_newest_completed_worker() {
    let launches = vec![
        LaunchRecord {
            key: "key-old".to_owned(),
            recipient: "worker-old".to_owned(),
            scope: "Audit rust".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "completed".to_owned(),
        },
        LaunchRecord {
            key: "key-new".to_owned(),
            recipient: "worker-new".to_owned(),
            scope: "Audit rust".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "completed".to_owned(),
        },
    ];
    let args = json!({"prompt": "Audit rust", "claudex_model": "gpt-test"});
    let result = find_reusable_launch(&launches, &args);
    assert_eq!(
        result.map(|r| r.recipient.as_str()),
        Some("worker-new"),
        "same-priority completed workers must resume the newest transcript/cache"
    );
}

#[test]
fn model_less_placeholder_does_not_block_explicit_model_fanout() {
    let launches = vec![LaunchRecord {
        key: "pending".to_owned(),
        recipient: String::new(),
        scope: "Recover post-reboot state".to_owned(),
        model: None,
        status: "active".to_owned(),
    }];
    assert!(
        !scope_is_occupied(&launches, "recover post-reboot state", Some("gpt-test")),
        "explicit model A must still fan out beside a model-less placeholder"
    );
    assert!(
        !scope_is_occupied(&launches, "recover post-reboot state", Some("cursor-test")),
        "explicit model B must still fan out beside a model-less placeholder"
    );
    assert!(
        scope_is_occupied(&launches, "recover post-reboot state", None),
        "model-less queries still collide with any same-scope occupant"
    );
}

#[test]
fn merge_record_refreshes_scope_on_explicit_resume_follow_up() {
    let mut launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit rust".to_owned(),
        model: Some("gpt-test".to_owned()),
        status: "completed".to_owned(),
    }];
    super::records::merge_launches(
        &mut launches,
        std::iter::once(&LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Extend the audit to CSS".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "active".to_owned(),
        }),
    );
    assert_eq!(launches[0].scope, "Extend the audit to CSS");
    assert_eq!(launches[0].status, "active");
    let follow = json!({
        "prompt":"Extend the audit to CSS",
        "claudex_model":"gpt-test"
    });
    assert_eq!(
        find_reusable_launch(&launches, &follow).map(|r| r.recipient.as_str()),
        Some("worker-a")
    );
}

#[test]
fn apply_transcript_merge_same_recipient() {
    let mut launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: String::new(),
        model: None,
        status: "active".to_owned(),
    }];
    let messages = vec![
        json!({
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "tool-a",
                "name": "Agent",
                "input": {"prompt": "New scope"}
            }]
        }),
        json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "tool-a",
                "content": [{"type": "text", "text": "Async agent launched successfully.\nagentId: worker-a"}]
            }]
        }),
    ];
    apply_transcript(&mut launches, &messages);
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].scope, "New scope");
}

#[test]
fn queued_message_recipient_extraction() {
    let mut launches = vec![LaunchRecord {
        key: "key-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Test".to_owned(),
        model: Some("model".to_owned()),
        status: "active".to_owned(),
    }];
    let messages = vec![json!({
        "role": "user",
        "content": [{"type": "tool_result", "tool_use_id": "send-a", "content": "Agent \"worker-a\" had no active task; resumed from transcript in the background."}]
    })];
    apply_transcript(&mut launches, &messages);
    assert_eq!(launches[0].status, "message_queued");
}

#[test]
fn scope_is_occupied_empty_scope_key() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Test".to_owned(),
        model: None,
        status: "active".to_owned(),
    }];
    assert!(!scope_is_occupied(&launches, "", None));
}

#[test]
fn scope_is_occupied_terminal_status_ignored() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Test scope".to_owned(),
        model: None,
        status: "completed".to_owned(),
    }];
    assert!(!scope_is_occupied(&launches, "test scope", None));
}

#[test]
fn note_inflight_launch_empty_scope() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({"prompt": "", "claudex_model": "model"});
    registry.note_inflight_launch("session-a", &arguments, "tool-a");
    assert!(
        registry
            .state_for("session-a")
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn memory_claim_occupancy_checks_expiry_session_scope_and_model() {
    let registry = SubagentReuseRegistry::default();
    let now = unix_seconds();
    {
        let mut claims = registry.claims.lock().expect("claims lock");
        claims.insert(
            "expired".to_owned(),
            super::store::ClaimRecord {
                session_id: "session-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("model-a".to_owned()),
                owner: "owner".to_owned(),
                pid: current_pid(),
                created_revision: 1,
                expires_unix_seconds: now.saturating_sub(1),
                tool_use_id: "expired".to_owned(),
            },
        );
        claims.insert(
            "wrong-session".to_owned(),
            super::store::ClaimRecord {
                session_id: "session-b".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("model-a".to_owned()),
                owner: "owner".to_owned(),
                pid: current_pid(),
                created_revision: 2,
                expires_unix_seconds: now.saturating_add(60),
                tool_use_id: "wrong-session".to_owned(),
            },
        );
        claims.insert(
            "wrong-scope".to_owned(),
            super::store::ClaimRecord {
                session_id: "session-a".to_owned(),
                scope: "Review CSS".to_owned(),
                model: Some("model-a".to_owned()),
                owner: "owner".to_owned(),
                pid: current_pid(),
                created_revision: 3,
                expires_unix_seconds: now.saturating_add(60),
                tool_use_id: "wrong-scope".to_owned(),
            },
        );
        claims.insert(
            "wrong-model".to_owned(),
            super::store::ClaimRecord {
                session_id: "session-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("model-b".to_owned()),
                owner: "owner".to_owned(),
                pid: current_pid(),
                created_revision: 4,
                expires_unix_seconds: now.saturating_add(60),
                tool_use_id: "wrong-model".to_owned(),
            },
        );
    }
    let arguments = json!({"prompt": "Audit Rust", "claudex_model": "model-a"});
    assert!(!registry.scope_is_occupied("session-a", &arguments));
    let model_less = json!({"prompt": "Audit Rust"});
    assert!(registry.scope_is_occupied("session-a", &model_less));
}

#[test]
fn find_reusable_launch_no_exact_match_returns_none() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit rust tests".to_owned(),
        model: Some("model-1".to_owned()),
        status: "active".to_owned(),
    }];
    let args = json!({"prompt": "Review CSS", "claudex_model": "model-1"});
    assert!(find_reusable_launch(&launches, &args).is_none());
}

#[test]
fn latest_user_text_prefers_latest_user_message() {
    let messages = vec![
        json!({"role": "user", "content": [{"type": "text", "text": "first"}]}),
        json!({"role": "assistant", "content": [{"type": "text", "text": "response"}]}),
        json!({"role": "user", "content": [{"type": "text", "text": "second"}]}),
    ];
    assert_eq!(latest_user_text(&messages), "second");
}

#[test]
fn scope_similarity_case_insensitive() {
    let sim1 = scope_similarity("Audit RUST Adapter", "audit rust tests");
    let sim2 = scope_similarity("audit rust adapter", "audit rust tests");
    assert_eq!(sim1, sim2);
}

#[test]
fn apply_transcript_empty_content() {
    let mut launches = vec![];
    let messages = vec![json!({"role": "user", "content": []})];
    apply_transcript(&mut launches, &messages);
    assert!(launches.is_empty());
}

#[test]
fn find_reusable_launch_with_model_none() {
    let launches = vec![LaunchRecord {
        key: "key-1".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Test scope".to_owned(),
        model: None,
        status: "active".to_owned(),
    }];
    let args = json!({"prompt": "Test scope"});
    let result = find_reusable_launch(&launches, &args);
    assert!(result.is_some());
}

#[test]
fn scope_similarity_minimum_word_length_filter() {
    let similarity = scope_similarity("ab cd audit", "audit test");
    assert!(similarity == 1);
}

#[test]
fn merge_launches_keeps_status_when_observed_status_is_blank() {
    let mut launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit rust".to_owned(),
        model: Some("gpt-test".to_owned()),
        status: "active".to_owned(),
    }];
    super::records::merge_launches(
        &mut launches,
        std::iter::once(&LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Audit rust".to_owned(),
            model: None,
            status: String::new(),
        }),
    );
    assert_eq!(launches[0].status, "active");
    assert_eq!(launches[0].model.as_deref(), Some("gpt-test"));
}

#[test]
fn merge_launches_does_not_collapse_empty_key_recipient_placeholders() {
    let mut launches = vec![LaunchRecord {
        key: String::new(),
        recipient: String::new(),
        scope: "Audit rust tests".to_owned(),
        model: Some("model-a".to_owned()),
        status: "active".to_owned(),
    }];
    super::records::merge_launches(
        &mut launches,
        std::iter::once(&LaunchRecord {
            key: String::new(),
            recipient: String::new(),
            scope: "Review CSS styles".to_owned(),
            model: Some("model-b".to_owned()),
            status: "active".to_owned(),
        }),
    );
    assert_eq!(
        launches.len(),
        2,
        "parallel inflight placeholders must stay distinct"
    );
}

#[test]
fn apply_transcript_skips_messages_without_content_and_unknown_status_ids() {
    let mut launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit rust".to_owned(),
        model: None,
        status: "active".to_owned(),
    }];
    apply_transcript(
        &mut launches,
        &[
            json!({"role":"user"}),
            json!({"role":"assistant","content":[{"type":"tool_use","name":"Agent"}]}),
            json!({"role":"assistant","content":[{"type":"tool_use","id":"tool-b"}]}),
            json!({
                "role":"user",
                "content":"<task-id>missing-task</task-id><status>failed</status>"
            }),
            json!({
                "role":"user",
                "content":"Agent \"ghost-worker\" had no active task"
            }),
        ],
    );
    assert_eq!(launches[0].status, "active");
    assert_eq!(launches.len(), 1);
}

#[test]
fn apply_transcript_marks_status_by_launch_key() {
    let mut launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit rust".to_owned(),
        model: None,
        status: "active".to_owned(),
    }];
    apply_transcript(
        &mut launches,
        &[json!({
            "role":"user",
            "content":"<task-id>tool-a</task-id><status>timeout</status>"
        })],
    );
    assert_eq!(launches[0].status, "timeout");
}

#[test]
fn live_agent_task_ids_dedupes_matching_key_and_recipient() {
    let messages = vec![json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"a4496564387a2561f",
            "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
        }]
    })];
    assert_eq!(
        live_agent_task_ids(&messages),
        vec!["a4496564387a2561f".to_owned()]
    );
}

#[test]
fn rewrite_launch_input_skips_empty_recipient() {
    let registry = SubagentReuseRegistry::default();
    let messages = vec![
        json!({"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Task","input":{"prompt":"test"}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"launching"}]}),
    ];
    let mut request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::String("".to_owned()),
        messages,
        tools: vec![],
        stream: false,
        output_config: Value::Null,
        metadata: json!({"_claudex_transport_identity":{"session_id":"sess-1"}}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    registry.observe_and_restore(&mut request);

    // Simulate a pending launch record with empty recipient
    let mut states = registry.states.lock().unwrap();
    states.entry("sess-1".to_owned()).or_insert_with(|| {
        SessionState {
            launches: vec![LaunchRecord {
                key: "t1".to_owned(),
                recipient: String::new(), // EMPTY recipient (pending)
                scope: "test".to_owned(),
                model: Some("model-1".to_owned()),
                status: "pending".to_owned(),
            }],
        }
    });
    drop(states);

    // Try to rewrite launch - should return None due to empty recipient
    let mut arguments = json!({"prompt":"test","claudex_model":"model-1"});
    let result = registry.rewrite_launch_input("sess-1", &mut arguments);

    assert!(
        result.is_none(),
        "should not inject resume for empty recipient (pending launch)"
    );
    assert_eq!(
        arguments.get("resume"),
        None,
        "resume field should not be added"
    );
}

#[test]
fn inflight_placeholder_receives_recipient_so_resume_rewrite_works() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Start with Vibrato boundaries.",
        "claudex_model":"gpt-test"
    });
    registry.note_inflight_launch("session-a", &arguments, "tool-pending");
    let mut request = request(
        "session-a",
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-pending",
                    "name":"Agent",
                    "input":arguments
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"tool-pending",
                    "content":"Async agent launched successfully.\nagentId: worker-inflight"
                }]
            }),
        ],
    );
    registry.observe_and_restore(&mut request);
    let mut follow_up = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Continue with the Vibrato path.",
        "claudex_model":"gpt-test"
    });
    assert_eq!(
        registry.rewrite_launch_input("session-a", &mut follow_up),
        Some("worker-inflight".to_owned()),
        "launch tool_result must fill the inflight empty recipient for resume/prompt-cache"
    );
    assert_eq!(follow_up["resume"], "worker-inflight");
}

#[test]
fn reuse_guidance_omits_empty_inflight_and_failed_recipients() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description":"Trace azookey conversion pipeline",
        "prompt":"Start with Vibrato boundaries.",
        "claudex_model":"gpt-test"
    });
    registry.note_inflight_launch("session-a", &arguments, "tool-pending");
    {
        let mut states = registry.states.lock().expect("lock");
        let state = states.get_mut("session-a").expect("session");
        state.launches.push(LaunchRecord {
            key: "tool-failed".to_owned(),
            recipient: "worker-failed".to_owned(),
            scope: "Trace azookey conversion pipeline".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "failed".to_owned(),
        });
        state.launches.push(LaunchRecord {
            key: "tool-live".to_owned(),
            recipient: "worker-live".to_owned(),
            scope: "Trace azookey conversion pipeline".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "completed".to_owned(),
        });
    }
    let mut follow_up = request(
        "session-a",
        vec![json!({"role":"user","content":"Continue the azookey conversion pipeline"})],
    );
    registry.observe_and_restore(&mut follow_up);
    let guidance = follow_up.system.to_string();
    assert!(
        guidance.contains(REUSE_GUIDANCE_MARKER),
        "completed workers must still restore reuse guidance"
    );
    assert!(
        guidance.contains("worker-live"),
        "reusable completed worker must appear in guidance: {guidance}"
    );
    assert!(
        !guidance.contains("worker-failed"),
        "failed workers must not be resume targets: {guidance}"
    );
    assert!(
        !guidance.contains("(Trace azookey conversion pipeline; gpt-test; pending)"),
        "empty-recipient inflight placeholders must not appear as resume targets: {guidance}"
    );
}

fn reuse_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn empty_session_id_is_rejected_by_delta_and_delete() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    assert!(
        !store
            .save_session_delta("", SessionState::default(), 0)
            .expect("empty session delta")
    );
    assert!(!store.delete_session("", 0).expect("empty session delete"));
}

#[test]
fn save_skips_a_tombstoned_session() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    assert!(
        store
            .delete_session("session-a", 0)
            .expect("tombstone a session that never existed")
    );
    let mut states = HashMap::new();
    states.insert(
        "session-a".to_owned(),
        SessionState {
            launches: vec![LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                status: "active".to_owned(),
            }],
        },
    );
    store.save(states).expect("save after tombstone");
    assert!(
        !store.load_snapshot().sessions.contains_key("session-a"),
        "a tombstoned session must not be resurrected by save()"
    );
}

#[test]
fn delete_session_rejects_a_stale_base_revision() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let mut initial = HashMap::new();
    initial.insert(
        "session-a".to_owned(),
        SessionState {
            launches: vec![LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                status: "active".to_owned(),
            }],
        },
    );
    store.save(initial).expect("initial snapshot");
    let stale_base_revision = store
        .load_snapshot()
        .session_revisions
        .get("session-a")
        .copied()
        .expect("session revision");
    store
        .save_session_delta(
            "session-a",
            SessionState {
                launches: vec![LaunchRecord {
                    key: "tool-b".to_owned(),
                    recipient: "worker-b".to_owned(),
                    scope: "Audit Rust".to_owned(),
                    model: Some("worker-model".to_owned()),
                    status: "active".to_owned(),
                }],
            },
            stale_base_revision,
        )
        .expect("advance revision past the captured base");

    let deleted = store
        .delete_session("session-a", stale_base_revision)
        .expect("stale delete check");
    assert!(
        !deleted,
        "a stale base revision must not delete a session that moved on"
    );
    assert!(store.load_snapshot().sessions.contains_key("session-a"));
}

#[test]
fn save_session_delta_merges_into_a_session_with_a_newer_untombstoned_revision() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let mut initial = HashMap::new();
    initial.insert(
        "session-a".to_owned(),
        SessionState {
            launches: vec![LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                status: "completed".to_owned(),
            }],
        },
    );
    store.save(initial).expect("initial snapshot");
    let captured_base_revision = store
        .load_snapshot()
        .session_revisions
        .get("session-a")
        .copied()
        .expect("session revision");
    // Advance the canonical revision again without tombstoning the session,
    // so a delta still carrying `captured_base_revision` becomes stale but
    // must merge instead of being rejected outright.
    store
        .save_session_delta(
            "session-a",
            SessionState {
                launches: vec![LaunchRecord {
                    key: "tool-b".to_owned(),
                    recipient: "worker-b".to_owned(),
                    scope: "Audit Rust".to_owned(),
                    model: Some("worker-model".to_owned()),
                    status: "active".to_owned(),
                }],
            },
            captured_base_revision,
        )
        .expect("advance revision");

    let merged = store
        .save_session_delta(
            "session-a",
            SessionState {
                launches: vec![LaunchRecord {
                    key: "tool-c".to_owned(),
                    recipient: "worker-c".to_owned(),
                    // A distinct scope keeps this from collapsing into the
                    // still-active "tool-b" record as the same logical
                    // worker; the merge must add it as a new launch.
                    scope: "Review CSS".to_owned(),
                    model: Some("worker-model".to_owned()),
                    status: "active".to_owned(),
                }],
            },
            captured_base_revision,
        )
        .expect("stale but mergeable delta");
    assert!(
        merged,
        "a stale, non-tombstoned delta must still be accepted via merge"
    );
    let launches = store.load_snapshot().sessions["session-a"].launches.clone();
    assert!(launches.iter().any(|launch| launch.key == "tool-b"));
    assert!(launches.iter().any(|launch| launch.key == "tool-c"));
}

#[test]
fn write_document_reports_an_error_when_the_target_path_is_a_directory() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    fs::create_dir(&path).expect("occupy the store path with a directory");
    let store = Store::new(path);
    let result = store.save_session_delta("session-a", SessionState::default(), 0);
    assert!(
        result.is_err(),
        "the atomic rename over an occupied directory must fail"
    );
}

#[test]
fn acquire_claim_rejects_empty_session_scope_or_owner() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let now = unix_seconds();
    let base = ClaimRequest {
        session_id: "session-a".to_owned(),
        scope: "Audit Rust".to_owned(),
        model: None,
        owner: "owner-a".to_owned(),
        pid: current_pid(),
        tool_use_id: "tool-a".to_owned(),
        expires_unix_seconds: now + 60,
    };
    let mut empty_session = base.clone();
    empty_session.session_id = String::new();
    assert!(
        store
            .acquire_claim(empty_session, now)
            .expect("empty session_id")
            .is_none()
    );
    let mut empty_scope = base.clone();
    empty_scope.scope = String::new();
    assert!(
        store
            .acquire_claim(empty_scope, now)
            .expect("empty scope")
            .is_none()
    );
    let mut empty_owner = base;
    empty_owner.owner = String::new();
    assert!(
        store
            .acquire_claim(empty_owner, now)
            .expect("empty owner")
            .is_none()
    );
}

#[test]
fn claims_occupy_ignores_other_sessions_and_scopes() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let now = unix_seconds();
    store
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                owner: "owner-a".to_owned(),
                pid: current_pid(),
                tool_use_id: "tool-a".to_owned(),
                expires_unix_seconds: now + 60,
            },
            now,
        )
        .expect("claim admission")
        .expect("claim");

    assert!(
        !store
            .claims_occupy("session-b", "Audit Rust", Some("worker-model"), now)
            .expect("other session")
    );
    assert!(
        !store
            .claims_occupy("session-a", "Review CSS", Some("worker-model"), now)
            .expect("other scope")
    );
    assert!(
        store
            .claims_occupy("session-a", "Audit Rust", Some("worker-model"), now)
            .expect("matching claim")
    );
}

#[test]
fn claims_occupy_reaps_a_definitely_dead_nonzero_pid() {
    let mut child = Command::new("true")
        .spawn()
        .expect("spawn a short-lived helper process");
    let dead_pid = child.id();
    child
        .wait()
        .expect("reap the helper process so its pid is no longer alive");

    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path);
    let now = unix_seconds();
    store
        .acquire_claim(
            ClaimRequest {
                session_id: "session-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                owner: "owner-a".to_owned(),
                pid: dead_pid,
                tool_use_id: "tool-dead-pid".to_owned(),
                expires_unix_seconds: now + 60,
            },
            now,
        )
        .expect("claim admission");

    assert!(
        !store
            .claims_occupy("session-a", "Audit Rust", Some("worker-model"), now)
            .expect("dead pid reap")
    );
}

#[test]
fn note_inflight_launch_rejects_an_in_memory_duplicate_without_a_store() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description": "Guard duplicate admission",
        "prompt": "Reject a second same-scope launch before any tool_result exists.",
        "claudex_model": "gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-first"));
    assert!(
        !registry.note_inflight_launch("session-a", &arguments, "tool-second"),
        "a same-scope in-memory claim must block a concurrent duplicate"
    );
}

#[test]
fn note_inflight_launch_returns_false_when_reuse_is_disabled() {
    let _guard = reuse_env_lock();
    let previous = std::env::var_os(crate::parallel_scheduler::SUBAGENT_REUSE_ENV);
    unsafe {
        std::env::set_var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV, "false");
    }
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description": "Trace disabled reuse",
        "prompt": "Confirm the flag short-circuits before any claim work.",
        "claudex_model": "gpt-test"
    });
    let admitted = registry.note_inflight_launch("session-a", &arguments, "tool-disabled");
    match previous {
        Some(value) => unsafe {
            std::env::set_var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV, value)
        },
        None => unsafe { std::env::remove_var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV) },
    }
    assert!(!admitted);
    assert_eq!(registry.state_for("session-a"), None);
}

#[test]
fn scope_is_occupied_falls_back_to_in_memory_claims_without_a_store() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description": "Guard duplicate admission",
        "prompt": "Fall back to the claim-only view once in-memory launch state is forgotten.",
        "claudex_model": "gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-claim-only"));
    registry.forget_memory_for_test();
    assert!(
        registry.scope_is_occupied("session-a", &arguments),
        "an in-memory claim must still occupy the scope without a store"
    );
    let other = json!({
        "description": "Unrelated different scope entirely",
        "prompt": "This scope must not match the claim above.",
        "claudex_model": "gpt-test"
    });
    assert!(!registry.scope_is_occupied("session-a", &other));
}

#[test]
fn resolve_claims_removes_the_local_mirror_after_a_store_backed_release() {
    let root = tempfile::tempdir().expect("reuse store");
    let path = root.path().join("subagent-recipients-v1.json");
    let registry = SubagentReuseRegistry::with_store(path);
    let arguments = json!({
        "description": "Trace resolve_claims store-backed release",
        "prompt": "Confirm the local claim mirror is dropped once the store admits the resolution.",
        "claudex_model": "gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-pending"));
    assert_eq!(registry.claims.lock().expect("claims lock").len(), 1);

    let mut resolved_request =
        request("session-a", launch_with_context("tool-pending", "worker-a"));
    registry.observe_and_restore_for_test(&mut resolved_request, true);

    assert!(
        registry.claims.lock().expect("claims lock").is_empty(),
        "a resolved store-backed claim must drop its local mirror"
    );
}

#[test]
fn note_inflight_launch_returns_false_when_the_store_cannot_acquire_a_claim() {
    let root = tempfile::tempdir().expect("reuse store fixture");
    let not_a_dir = root.path().join("not-a-dir");
    fs::write(&not_a_dir, "x").expect("file where directory should be");
    let failing = SubagentReuseRegistry::with_store(not_a_dir.join("cache.json"));
    let arguments = json!({
        "description": "Trace a broken store path",
        "prompt": "acquire_claim must fail when the store directory cannot be created.",
        "claudex_model": "gpt-test"
    });
    assert!(
        !failing.note_inflight_launch("session-a", &arguments, "tool-broken-store"),
        "a store that cannot acquire a claim must not admit the launch"
    );
    assert!(
        failing.claims.lock().expect("claims lock").is_empty(),
        "a failed store acquisition must not leave a local claim behind"
    );
    assert_eq!(failing.state_for("session-a"), None);
}

#[test]
fn scope_is_occupied_ignores_a_model_less_claim_for_an_explicit_model_query() {
    let registry = SubagentReuseRegistry::default();
    let now = unix_seconds();
    registry.claims.lock().expect("claims lock").insert(
        "model-less".to_owned(),
        super::store::ClaimRecord {
            session_id: "session-a".to_owned(),
            scope: "Audit Rust".to_owned(),
            model: None,
            owner: "owner".to_owned(),
            pid: current_pid(),
            created_revision: 1,
            expires_unix_seconds: now.saturating_add(60),
            tool_use_id: "model-less".to_owned(),
        },
    );
    let arguments = json!({"prompt": "Audit Rust", "claudex_model": "gpt-test"});
    assert!(
        !registry.scope_is_occupied("session-a", &arguments),
        "an explicit model query must not collide with a model-less stored claim"
    );
}

#[test]
fn note_inflight_launch_dedup_ignores_a_different_session() {
    let registry = SubagentReuseRegistry::default();
    let arguments = json!({
        "description": "Trace cross-session claim isolation",
        "prompt": "Two sessions launching the same scope must both be admitted.",
        "claudex_model": "gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-a"));
    assert!(
        registry.note_inflight_launch("session-b", &arguments, "tool-b"),
        "an identical scope in a different session must not be treated as a duplicate"
    );
    assert_eq!(registry.claims.lock().expect("claims lock").len(), 2);
}

#[test]
fn resolve_claims_keeps_a_still_pending_claim_in_the_local_mirror() {
    let root = tempfile::tempdir().expect("reuse store");
    let path = root.path().join("subagent-recipients-v1.json");
    let registry = SubagentReuseRegistry::with_store(path);
    let arguments = json!({
        "description": "Trace resolve_claims pending retention",
        "prompt": "A claim without a matching resolved launch must stay in the mirror.",
        "claudex_model": "gpt-test"
    });
    assert!(registry.note_inflight_launch("session-a", &arguments, "tool-pending"));
    assert_eq!(registry.claims.lock().expect("claims lock").len(), 1);

    // Still in flight: no tool_result yet, so the launch keeps an empty
    // recipient and a non-terminal status.
    let mut pending_request = request(
        "session-a",
        vec![json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":"tool-pending",
                "name":"Agent",
                "input":arguments
            }]
        })],
    );
    registry.observe_and_restore_for_test(&mut pending_request, true);

    assert_eq!(
        registry.claims.lock().expect("claims lock").len(),
        1,
        "an unresolved claim must remain until its launch reports a recipient or terminal status"
    );
}

#[test]
fn scope_is_occupied_true_via_store_backed_session_state_without_local_memory_or_claims() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let store = Store::new(path.clone());
    store
        .save_session_delta(
            "session-a",
            SessionState {
                launches: vec![LaunchRecord {
                    key: "tool-a".to_owned(),
                    recipient: "worker-a".to_owned(),
                    scope: "Audit Rust".to_owned(),
                    model: Some("worker-model".to_owned()),
                    status: "active".to_owned(),
                }],
            },
            0,
        )
        .expect("seed store-backed session state");

    let registry = SubagentReuseRegistry::with_store(path);
    let arguments = json!({"prompt": "Audit Rust", "claudex_model": "worker-model"});
    assert!(
        registry.scope_is_occupied("session-a", &arguments),
        "store-backed session state alone (no local memory, no claims) must occupy the scope"
    );
}

#[test]
fn persist_without_a_store_is_a_no_op() {
    let registry = SubagentReuseRegistry::default();
    registry.persist(HashMap::new());
}

#[test]
fn persist_with_a_store_writes_the_snapshot() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    let registry = SubagentReuseRegistry::with_store(path.clone());
    let mut states = HashMap::new();
    states.insert(
        "session-a".to_owned(),
        SessionState {
            launches: vec![LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit Rust".to_owned(),
                model: Some("worker-model".to_owned()),
                status: "active".to_owned(),
            }],
        },
    );
    registry.persist(states);
    let stored =
        serde_json::from_slice::<StoredStates>(&fs::read(path).expect("persisted registry"))
            .expect("valid registry JSON");
    assert!(stored.sessions.contains_key("session-a"));
}

#[test]
fn persist_with_a_store_that_cannot_save_does_not_panic() {
    let root = tempfile::tempdir().expect("reuse registry fixture");
    let path = root.path().join("reuse.json");
    fs::create_dir(&path).expect("occupy the store path with a directory");
    let registry = SubagentReuseRegistry::with_store(path);
    // The atomic rename onto an occupied directory fails; persist() must log
    // and swallow the error rather than panicking.
    registry.persist(HashMap::new());
}

#[test]
fn resolve_claims_keeps_the_local_mirror_when_the_store_release_fails() {
    let root = tempfile::tempdir().expect("reuse store fixture");
    let not_a_dir = root.path().join("not-a-dir");
    fs::write(&not_a_dir, "x").expect("file where directory should be");
    let failing = SubagentReuseRegistry::with_store(not_a_dir.join("cache.json"));
    let claim = super::store::ClaimRecord {
        session_id: "session-a".to_owned(),
        scope: "Audit Rust".to_owned(),
        model: Some("gpt-test".to_owned()),
        owner: "someone-else".to_owned(),
        pid: current_pid(),
        created_revision: 1,
        expires_unix_seconds: unix_seconds().saturating_add(60),
        tool_use_id: "tool-a".to_owned(),
    };
    failing
        .claims
        .lock()
        .expect("claims lock")
        .insert("tool-a".to_owned(), claim);
    let launches = vec![LaunchRecord {
        key: "tool-a".to_owned(),
        recipient: "worker-a".to_owned(),
        scope: "Audit Rust".to_owned(),
        model: Some("gpt-test".to_owned()),
        status: "completed".to_owned(),
    }];
    failing.resolve_claims("session-a", &launches);
    assert_eq!(
        failing.claims.lock().expect("claims lock").len(),
        1,
        "a store release failure must not drop the local claim mirror"
    );
}
