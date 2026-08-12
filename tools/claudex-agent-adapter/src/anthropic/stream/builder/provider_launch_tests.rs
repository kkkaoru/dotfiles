use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

use super::*;
use crate::agent_backend::AgentBackend;
use crate::anthropic::{Bridge, Session};

fn test_session(claude_session_id: Option<&str>) -> Session {
    let slots = Arc::new(Semaphore::new(1));
    Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::from([("Agent".to_owned(), "Agent".to_owned())]),
        client_user_id: None,
        claude_session_id: claude_session_id.map(str::to_owned),
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    }
}

#[tokio::test]
async fn report_incomplete_launches_emits_visible_warning() {
    let mut builder = SegmentBuilder::new(1);
    builder
        .incomplete_launch_call_ids
        .push("incomplete-1".to_owned());
    builder
        .report_incomplete_launches(None)
        .await
        .expect("incomplete report");
    assert!(
        builder.incomplete_launch_call_ids.is_empty(),
        "reported incomplete launches must clear"
    );
}

#[tokio::test]
async fn drain_remaining_queued_launches_bridges_session_queue() {
    let root = tempfile::tempdir().expect("launch queue fixture");
    let queue_dir = root.path().join("cache");
    std::fs::create_dir_all(&queue_dir).expect("queue dir");
    let queue = queue_dir.join("launch-queue.jsonl");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs_f64();
    // Session-owned queues live beside the base path as launch-queue.<owner>.jsonl.
    let owned = crate::launch_mcp::launch_queue_path(&queue_dir, Some("scope-a"));
    std::fs::write(
        &owned,
        format!(
            "{}\n",
            json!({
                "ts": now,
                "name": "Agent",
                "owner": "scope-a",
                "arguments": {"prompt": "queued worker"}
            })
        ),
    )
    .expect("queue entry");
    let _guard = temporary_env::set_var("CLAUDEX_LAUNCH_QUEUE", &queue);

    // Use a settled Copilot leaf so tool_call rejection is not required to
    // exercise drain / queued_launch_tool_call / record paths.
    let leaf = Arc::new(AgentBackend::Copilot(
        crate::copilot_acp::CopilotAcp::settled_for_test().await,
    ));
    let backend = AgentBackend::routed(vec![("main".to_owned(), leaf)]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-a"));
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .drain_remaining_queued_launches(&bridge, &session, &[], &json!(null), None)
        .await;
    assert!(
        !builder.bridged_provider_launch_ids.is_empty(),
        "queued Agent must bridge into a provider launch id before tool routing"
    );
}

#[tokio::test]
async fn provider_launch_tracks_unbridged_mcp_cards() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-b"));
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &json!({
                "method": "item/providerTool/call",
                "params": {
                    "callId": "mcp-empty",
                    "tool": "mcp",
                    "title": "MCP",
                    "status": "pending",
                    "arguments": {}
                }
            }),
            None,
        )
        .await
        .expect("mcp card");
    assert!(
        builder
            .mcp_provider_call_ids
            .iter()
            .any(|id| id == "mcp-empty")
    );
    assert!(
        builder
            .incomplete_launch_call_ids
            .iter()
            .any(|id| id == "mcp-empty")
    );
}

#[tokio::test]
async fn provider_launch_forwards_non_mcp_call_and_update_statuses() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let session = test_session(Some("scope-c"));
    let mut builder = SegmentBuilder::new(1);

    for (method, status) in [
        ("item/providerTool/call", "pending"),
        ("item/providerTool/update", "in_progress"),
        ("item/providerTool/update", "completed"),
        ("item/providerTool/update", "failed"),
        ("item/providerTool/update", "unknown"),
    ] {
        builder
            .provider_launch_event(
                &bridge,
                &session,
                &[],
                &json!(null),
                &json!({
                    "method": method,
                    "params": {
                        "callId": "ordinary-call",
                        "tool": "shell",
                        "title": "ordinary tool",
                        "status": status,
                        "arguments": {}
                    }
                }),
                None,
            )
            .await
            .expect("ordinary provider event");
    }
}

mod temporary_env {
    use std::{
        path::Path,
        sync::{Mutex, OnceLock},
    };

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub(super) struct Guard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    pub(super) fn set_var(key: &'static str, value: &Path) -> Guard {
        let lock = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock");
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Guard {
            key,
            previous,
            _lock: lock,
        }
    }
}
