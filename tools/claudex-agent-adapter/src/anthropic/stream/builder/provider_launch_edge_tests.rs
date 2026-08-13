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

async fn settled_bridge() -> Bridge {
    let leaf = Arc::new(AgentBackend::Copilot(
        crate::copilot_acp::CopilotAcp::settled_for_test().await,
    ));
    let backend = AgentBackend::routed(vec![("main".to_owned(), leaf)]);
    Bridge::new_with_backend(backend, "main".to_owned())
}

fn agent_event(call_id: Option<&str>, prompt: Option<&str>) -> serde_json::Value {
    let mut params = json!({
        "tool": "Agent",
        "title": "Agent",
        "status": "pending",
        "arguments": prompt.map(|prompt| json!({"prompt": prompt})).unwrap_or(json!({}))
    });
    if let Some(call_id) = call_id {
        params["callId"] = json!(call_id);
    }
    json!({"method": "item/providerTool/call", "params": params})
}

#[tokio::test]
async fn provider_launch_bridges_mcp_and_deduplicates_the_same_call() {
    let bridge = settled_bridge().await;
    let session = test_session(Some("scope-mcp"));
    let mut builder = SegmentBuilder::new(1);
    builder
        .bridged_provider_launch_ids
        .push("mcp-ready".to_owned());
    let event = json!({
        "method": "item/providerTool/call",
        "params": {
            "callId": "mcp-ready",
            "tool": "mcp",
            "title": "MCP Agent",
            "status": "pending",
            "arguments": {"prompt": "queued worker"}
        }
    });
    builder
        .provider_launch_event(&bridge, &session, &[], &json!(null), &event, None)
        .await
        .expect("duplicate mcp launch");
    assert_eq!(builder.bridged_provider_launch_ids, ["mcp-ready"]);
    assert!(
        builder.mcp_provider_call_ids.is_empty(),
        "successful MCP bridge must not fall through to unbridged MCP tracking"
    );
}

#[tokio::test]
async fn unbridged_launch_without_a_call_id_is_still_suppressed() {
    let bridge = settled_bridge().await;
    let session = test_session(Some("scope-none"));
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_launch_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &agent_event(None, None),
            None,
        )
        .await
        .expect("missing call id");
    assert!(builder.incomplete_launch_call_ids.is_empty());
}

#[tokio::test]
async fn drain_skips_queued_entries_without_a_prompt() {
    let root = tempfile::tempdir().expect("launch queue fixture");
    let queue_dir = root.path().join("cache");
    std::fs::create_dir_all(&queue_dir).expect("queue dir");
    let queue = queue_dir.join("launch-queue.jsonl");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs_f64();
    let owned = crate::launch_mcp::launch_queue_path(&queue_dir, Some("scope-empty"));
    std::fs::write(
        &owned,
        format!(
            "{}\n",
            json!({
                "ts": now,
                "name": "Agent",
                "owner": "scope-empty",
                "arguments": {"_toolName": "Agent"}
            })
        ),
    )
    .expect("empty queue entry");
    let _guard = temporary_env::set_var("CLAUDEX_LAUNCH_QUEUE", &queue);
    let bridge = settled_bridge().await;
    let session = test_session(Some("scope-empty"));
    let mut builder = SegmentBuilder::new(1);
    builder
        .drain_remaining_queued_launches(&bridge, &session, &[], &json!(null), None)
        .await
        .expect("drain empty prompt");
    assert!(builder.bridged_provider_launch_ids.is_empty());
    let leftover = std::fs::read_to_string(&owned).unwrap_or_default();
    assert!(
        leftover.trim().is_empty(),
        "unbridgeable queue entries must still be consumed: {leftover}"
    );
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
