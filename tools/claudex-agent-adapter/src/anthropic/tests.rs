use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::body::to_bytes;
use serde_json::{Value, json};

use super::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS;
use super::{
    BRIDGE_INSTRUCTIONS, MessagesRequest, SUBAGENT_RESULT_PROTOCOL, Segment, Session,
    SignaturePool, Usage, WebEvidenceSummary,
    content::*,
    intern_signature,
    retention::{record_pending_tool, sweep_idle_sessions_at, take_oldest_evictable_at},
    session::{codex_tool_name, dynamic_tool},
    stream::send_stream_frame,
    trace_request,
    turn_input::{full_transcript_input, user_input_from_messages},
};

#[test]
fn bridge_requires_atomic_parallel_subagent_launches() {
    assert_bridge_launch_contract();
    assert_subagent_result_protocol_contract();
    assert_shared_workspace_contract();
}

fn assert_bridge_launch_contract() {
    assert!(
        BRIDGE_INSTRUCTIONS
            .contains("one ordinary supplied Agent/Task tool call per intended worker")
    );
    assert!(BRIDGE_INSTRUCTIONS.contains("Never invent or request an adapter-only batch tool"));
    assert!(BRIDGE_INSTRUCTIONS.contains("exactly that many native launch calls"));
    assert!(
        BRIDGE_INSTRUCTIONS.contains("never invent adapter-only claudex_model or claudex_effort")
    );
    assert!(
        BRIDGE_INSTRUCTIONS
            .contains("A follow-up queued to a busy worker does not add parallel capacity")
    );
    assert!(BRIDGE_INSTRUCTIONS.contains("end the turn promptly with concise user-visible status"));
    assert!(BRIDGE_INSTRUCTIONS.contains("never a complete SubAgent answer"));
    assert!(BRIDGE_INSTRUCTIONS.contains("Never copy end-the-turn-with-status"));
    assert!(BRIDGE_INSTRUCTIONS.contains("Avoid serial heavy processing by one worker"));
    assert!(BRIDGE_INSTRUCTIONS.contains(
        "reuse compatible workers by setting resume to the exact prior Agent/Task recipient instead of churning processes"
    ));
    assert!(
        BRIDGE_INSTRUCTIONS
            .contains("invoke Claude Code's supplied dynamic SubAgent tool directly")
    );
}

fn assert_subagent_result_protocol_contract() {
    assert!(SUBAGENT_RESULT_PROTOCOL.contains("TaskOutput(task_id)"));
    assert!(SUBAGENT_RESULT_PROTOCOL.contains("never wait for every background task"));
    assert!(
        SUBAGENT_RESULT_PROTOCOL
            .contains("never automatically poll TaskList or TaskOutput on a timer")
    );
    assert!(SUBAGENT_RESULT_PROTOCOL.contains("accepting another user instruction"));
    assert!(SUBAGENT_RESULT_PROTOCOL.contains("never call TaskOutput or TaskGet merely to drain"));
    assert!(
        SUBAGENT_RESULT_PROTOCOL
            .contains("Do not send ordinary worker results or progress through SendMessage")
    );
    assert!(
        SUBAGENT_RESULT_PROTOCOL
            .contains("Treat <agent-message> and <task-notification> content as lifecycle hints")
    );
}

fn assert_shared_workspace_contract() {
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("explicitly disjoint file ownership"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("serialize mutations"));
    assert!(
        SHARED_WORKSPACE_INSTRUCTIONS.contains("File content has changed since it was last read")
    );
    assert!(
        SHARED_WORKSPACE_INSTRUCTIONS
            .contains("missing filesystem access or a provider region/opt-in restriction")
    );
}

#[test]
fn stale_snapshot_edit_errors_require_refresh_before_retry() {
    assert!(
        SHARED_WORKSPACE_INSTRUCTIONS
            .contains("When a tool reports `File content has changed since it was last read`")
    );
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("stop the stale edit"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("re-read the latest file"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("coordinate ownership"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("instead of retrying the same patch"));
}

#[test]
fn unavailable_worker_errors_are_rerouted_without_retry_churn() {
    assert!(
        SHARED_WORKSPACE_INSTRUCTIONS
            .contains("missing filesystem access or a provider region/opt-in restriction")
    );
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("mark that route unavailable for this turn"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("reroute once"));
    assert!(SHARED_WORKSPACE_INSTRUCTIONS.contains("do not churn retries"));
}

#[tokio::test]
async fn tolerates_a_closed_stream_receiver() {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    drop(receiver);
    send_stream_frame(Some(&sender), "test", || json!({"ok":true}))
        .await
        .expect("closed receiver is not an upstream error");
}

#[tokio::test]
async fn evicts_only_an_unowned_session_without_pending_tools() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(3));
    let pending = test_session(&semaphore, true);
    let active = test_session(&semaphore, false);
    let active_owner = Arc::clone(&active);
    let idle = test_session(&semaphore, false);
    let sessions = tokio::sync::Mutex::new(vec![pending, active, idle]);

    drop(take_oldest_evictable_at(&sessions, Instant::now()).await);

    let retained = sessions.lock().await;
    assert_eq!(retained.len(), 2);
    assert!(
        retained
            .iter()
            .any(|session| Arc::ptr_eq(session, &active_owner))
    );
    assert_eq!(semaphore.available_permits(), 1);
}

fn test_session(semaphore: &Arc<tokio::sync::Semaphore>, has_pending_tool: bool) -> Arc<Session> {
    test_session_at(semaphore, has_pending_tool, Instant::now())
}

fn test_session_at(
    semaphore: &Arc<tokio::sync::Semaphore>,
    has_pending_tool: bool,
    last_activity: Instant,
) -> Arc<Session> {
    let pending_tools = if has_pending_tool {
        HashMap::from([("toolu_test".to_owned(), json!(1))])
    } else {
        HashMap::new()
    };
    Arc::new(Session {
        thread_id: "thread-test".to_owned(),
        model: "main-model".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: tokio::sync::Mutex::new(Vec::new()),
        pending_tools: tokio::sync::Mutex::new(pending_tools),
        consumed_tool_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(tokio::sync::Mutex::new(())),
        last_activity: std::sync::Mutex::new(last_activity),
        pending_since: std::sync::Mutex::new(has_pending_tool.then_some(last_activity)),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: Arc::clone(semaphore).try_acquire_owned().unwrap(),
    })
}

#[tokio::test]
async fn capacity_eviction_preserves_fresh_pending_and_active_sessions() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(2));
    let now = Instant::now();
    let active = test_session_at(&semaphore, true, now - Duration::from_secs(31 * 60));
    let active_owner = Arc::clone(&active);
    let fresh_activity = now - Duration::from_secs(29 * 60);
    let fresh = test_session_at(&semaphore, true, fresh_activity);
    let sessions = tokio::sync::Mutex::new(vec![active, fresh]);

    assert!(take_oldest_evictable_at(&sessions, now).await.is_none());

    let retained = sessions.lock().await;
    assert_eq!(retained.len(), 2);
    assert!(
        retained
            .iter()
            .any(|session| Arc::ptr_eq(session, &active_owner))
    );
    assert!(
        retained
            .iter()
            .any(|session| { *session.last_activity.lock().unwrap() == fresh_activity })
    );
}

#[tokio::test]
async fn starts_pending_ttl_when_the_external_tool_is_emitted() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let now = Instant::now();
    let session = test_session_at(&semaphore, false, now - Duration::from_secs(60 * 60));
    record_pending_tool(&session, "toolu_new".to_owned(), json!(7), now).await;
    assert_eq!(*session.last_activity.lock().unwrap(), now);
    *session.last_activity.lock().unwrap() = now - Duration::from_secs(60 * 60);
    let sessions = tokio::sync::Mutex::new(vec![session]);

    assert!(take_oldest_evictable_at(&sessions, now).await.is_none());
    assert_eq!(sessions.lock().await.len(), 1);
}

#[tokio::test]
async fn evicts_the_least_recently_used_idle_session() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(2));
    let now = Instant::now();
    let newer = test_session_at(&semaphore, false, now - Duration::from_secs(5));
    let older_activity = now - Duration::from_secs(10);
    let older = test_session_at(&semaphore, false, older_activity);
    let sessions = tokio::sync::Mutex::new(vec![newer, older]);

    let evicted = take_oldest_evictable_at(&sessions, now)
        .await
        .expect("an idle session should be evicted");
    assert_eq!(*evicted.last_activity.lock().unwrap(), older_activity);
}

#[tokio::test]
async fn sweeps_only_expired_unowned_sessions_without_pending_tools() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
    let now = Instant::now();
    let expired = test_session_at(&semaphore, false, now - Duration::from_secs(121 * 60));
    let active = test_session_at(&semaphore, false, now - Duration::from_secs(121 * 60));
    let active_owner = Arc::clone(&active);
    let pending = test_session_at(&semaphore, true, now - Duration::from_secs(60 * 60));
    let fresh_activity = now - Duration::from_secs(119 * 60);
    let fresh = test_session_at(&semaphore, false, fresh_activity);
    let sessions = tokio::sync::Mutex::new(vec![expired, active, pending, fresh]);

    assert_eq!(sweep_idle_sessions_at(&sessions, now).await.len(), 1);

    let retained = sessions.lock().await;
    assert_eq!(retained.len(), 3);
    assert!(
        retained
            .iter()
            .any(|session| Arc::ptr_eq(session, &active_owner))
    );
    assert!(
        retained
            .iter()
            .any(|session| !session.pending_tools.try_lock().unwrap().is_empty())
    );
    assert!(retained.iter().any(|session| {
        *session.last_activity.lock().expect("session clock") == fresh_activity
    }));
}

#[test]
fn strips_cache_control_when_matching_transcripts() {
    let left = json!({"role":"user","content":[{"type":"text","text":"hi"}]});
    let right = json!({"role":"user","content":[{
        "type":"text","text":"hi","cache_control":{"type":"ephemeral"}
    }]});
    assert_eq!(canonical_value(&left), canonical_value(&right));
}

#[test]
fn converts_tool_results() {
    let messages = vec![json!({
        "role":"user",
        "content":[{"type":"tool_result","tool_use_id":"call_1","content":"ok"}]
    })];
    let results = collect_tool_results(&messages);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_use_id, "call_1");
    assert_eq!(results[0].content_items[0]["text"], "ok");
}

#[test]
fn converts_content_tools_names_and_prompts() {
    assert_eq!(content_text(&json!("plain")), "plain");
    assert_eq!(
        content_text(&json!([
            {"type":"text","text":"one"},
            {"type":"image"},
            {"type":"text","text":"two"}
        ])),
        "one\ntwo"
    );
    assert_eq!(content_text(&Value::Null), "");

    let tool = json!({"name":"mcp__server.tool","description":"desc"});
    let name = codex_tool_name("mcp__server.tool", 3);
    assert_eq!(name, "cc_mcp__server_tool_3");
    let spec = dynamic_tool(&tool, &name).expect("valid dynamic tool");
    assert_eq!(spec["name"], name);
    assert!(spec["description"].as_str().unwrap().contains("desc"));
    assert!(dynamic_tool(&json!({}), "cc_missing").is_none());
    assert_eq!(codex_tool_name(&"x".repeat(200), 7).len(), 128);
    assert_ne!(codex_tool_name("foo.bar", 0), codex_tool_name("foo_bar", 1));
}

#[test]
fn converts_transcripts_images_and_rich_tool_results() {
    let single = vec![json!({
        "role":"user",
        "content":[
            {"type":"text","text":"hello"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAA"}},
            {"type":"image","source":{"type":"url","url":"https://example.test/a.png"}},
            {"type":"unknown"}
        ]
    })];
    let input = full_transcript_input(&single);
    assert_eq!(input[0]["text"], "hello");
    assert_eq!(input[1]["url"], "data:image/png;base64,AAA");
    assert_eq!(input[2]["url"], "https://example.test/a.png");

    let history = vec![
        json!({"role":"user","content":"first"}),
        json!({"role":"assistant","content":"second"}),
    ];
    assert!(
        full_transcript_input(&history)[0]["text"]
            .as_str()
            .unwrap()
            .contains("role-tagged history")
    );
    assert_eq!(
        user_input_from_messages(&[json!({"role":"user","content":"text"})])[0]["text"],
        "text"
    );
    assert_eq!(
        user_input_from_messages(&[
            json!({"role":"assistant","content":"ignored"}),
            json!({"role":"user","content":null})
        ])[0]["text"],
        "Continue."
    );
    assert!(image_data_url(&json!({"source":{"type":"other"}})).is_none());

    let results = collect_tool_results(&[json!({
        "content":[
            {"type":"text","text":"skip"},
            {"type":"tool_result"},
            {
                "type":"tool_result", "tool_use_id":"rich", "is_error":true,
                "content":[
                    {"type":"text","text":"bad"},
                    {"type":"image","source":{"type":"url","url":"https://example.test/i"}},
                    {"type":"unknown"}
                ]
            },
            {"type":"tool_result","tool_use_id":"empty","content":null}
        ]
    })]);
    assert_eq!(results.len(), 2);
    assert!(results[0].is_error);
    assert_eq!(results[0].content_items[1]["type"], "inputImage");
    assert_eq!(results[1].content_items[0]["text"], "");
    assert!(collect_tool_results(&[json!({"content":"not-array"})]).is_empty());
}

#[tokio::test]
async fn builds_anthropic_json_and_error_responses() {
    let response = anthropic_response(
        Segment {
            blocks: vec![json!({"type":"text","text":"OK"})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens: 10,
                output_tokens: 2,
                web_search_requests: 0,
            },
            web_evidence: WebEvidenceSummary::default(),
        },
        "model",
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response["content"][0]["text"], "OK");
    assert_eq!(response["usage"]["input_tokens"], 10);
    assert_eq!(response["usage"]["output_tokens"], 2);
    assert!(response.get("metadata").is_none());

    let error = error_response(
        axum::http::StatusCode::BAD_REQUEST,
        anyhow::anyhow!("bad request"),
    );
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = to_bytes(error.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("bad request"));

    let terminal = error_response(
        axum::http::StatusCode::BAD_GATEWAY,
        anyhow::anyhow!("Missing environment variable: SAKANA_AI_API_KEY"),
    );
    assert_eq!(terminal.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = to_bytes(terminal.into_body(), usize::MAX).await.unwrap();
    let terminal: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(terminal["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn exposes_verified_web_evidence_metadata_in_non_stream_response() {
    let response = anthropic_response(
        Segment {
            blocks: Vec::new(),
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
                web_search_requests: 3,
            },
            web_evidence: WebEvidenceSummary::from_verified_count(3),
        },
        "model",
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let response: Value = serde_json::from_slice(&body).expect("Anthropic JSON");

    assert_eq!(
        response["usage"]["server_tool_use"]["web_search_requests"],
        3
    );
    assert_eq!(
        response["metadata"]["claudex"]["web_evidence"]["verified_count"],
        3
    );
    assert_eq!(
        response["metadata"]["claudex"]["web_evidence"]["evidence_class_counts"]["verified_retrieval"],
        3
    );
}

#[test]
fn extracts_signatures_and_counts() {
    let mut request: MessagesRequest = serde_json::from_value(json!({
        "system":"system",
        "messages":[{"role":"user","content":"hello"}],
        "tools":[]
    }))
    .unwrap();
    super::RequestIdentity::new(Some("session-a".to_owned()), None, None).attach(&mut request);
    let base_signature =
        request_signature(&request, Some("test-advisor"), Some("test-collaborator")).unwrap();
    assert!(base_signature.contains("test-advisor"));
    let mut other_transport = request.clone();
    super::RequestIdentity::new(Some("session-b".to_owned()), None, None)
        .attach(&mut other_transport);
    assert_ne!(
        base_signature,
        request_signature(
            &other_transport,
            Some("test-advisor"),
            Some("test-collaborator")
        )
        .unwrap()
    );
    let serialized_bytes = serde_json::to_string(&request.system).unwrap().len()
        + serde_json::to_string(&request.messages).unwrap().len()
        + serde_json::to_string(&request.tools).unwrap().len();
    let expected_tokens = token_count(&request);
    let mut other_directory = request;
    other_directory.working_directory = Some("/tmp/other-project".into());
    assert_ne!(
        base_signature,
        request_signature(
            &other_directory,
            Some("test-advisor"),
            Some("test-collaborator")
        )
        .unwrap()
    );
    other_directory.working_directory = None;
    other_directory
        .disabled_subagent_models
        .insert("gpt-5.6-sol".to_owned());
    assert_ne!(
        base_signature,
        request_signature(
            &other_directory,
            Some("test-advisor"),
            Some("test-collaborator")
        )
        .unwrap()
    );
    assert_eq!(expected_tokens, serialized_bytes.div_ceil(4));
    assert_eq!(canonical_value(&json!(5)), json!(5));
}

#[test]
fn traces_request_metadata_without_prompt_contents() {
    let request: MessagesRequest = serde_json::from_value(json!({
        "model":"trace-model", "stream":true, "system":"system",
        "messages":[{"role":"user","content":"secret"}],
        "tools":[{"name":"lookup"}], "output_config":{"effort":"high"}
    }))
    .expect("trace request");
    let info_subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::with_default(info_subscriber, || {
        assert!(!trace_request(&request));
    });
    let debug_subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::with_default(debug_subscriber, || {
        assert!(trace_request(&request));
    });
}

#[test]
fn shares_live_session_signatures_and_releases_dead_values() {
    let pool = SignaturePool::default();
    let first = intern_signature(&pool, "large-shared-signature".to_owned());
    let shared = intern_signature(&pool, "large-shared-signature".to_owned());
    assert!(Arc::ptr_eq(&first, &shared));
    let distinct = intern_signature(&pool, "different-signature".to_owned());
    assert!(!Arc::ptr_eq(&first, &distinct));

    drop(first);
    drop(shared);
    drop(distinct);
    let replacement = intern_signature(&pool, "large-shared-signature".to_owned());
    assert_eq!(replacement.as_ref(), "large-shared-signature");
    assert_eq!(pool.lock().unwrap().values().flatten().count(), 2);
}

#[test]
fn prunes_signature_buckets_after_the_bound_is_reached() {
    let pool = SignaturePool::default();
    let mut signatures = Vec::new();
    for index in 0..super::MAX_SIGNATURE_BUCKETS {
        signatures.push(intern_signature(&pool, format!("signature-{index}")));
    }
    let _trigger = intern_signature(&pool, "signature-trigger".to_owned());
    assert!(!signatures.is_empty());
}

#[test]
fn cleans_empty_signature_buckets_at_the_bound() {
    let pool = SignaturePool::default();
    {
        let mut buckets = pool.lock().expect("signature pool");
        buckets.extend((0..super::MAX_SIGNATURE_BUCKETS).map(|key| (key as u64, Vec::new())));
    }

    let value = intern_signature(&pool, "after-cleanup".to_owned());
    assert_eq!(value.as_ref(), "after-cleanup");
    assert_eq!(pool.lock().expect("signature pool").len(), 1);
}

#[test]
fn intern_signature_scans_past_hash_colliding_candidates() {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let pool = SignaturePool::default();
    let mut hasher = DefaultHasher::new();
    "keep-scanning".hash(&mut hasher);
    let key = hasher.finish();
    let decoy = Arc::<str>::from("decoy-signature");
    {
        let mut buckets = pool.lock().expect("signature pool");
        buckets.insert(key, vec![Arc::downgrade(&decoy)]);
    }
    let matched = intern_signature(&pool, "keep-scanning".to_owned());
    assert_eq!(matched.as_ref(), "keep-scanning");
    assert!(
        !Arc::ptr_eq(&matched, &decoy),
        "hash-bucket peers must not be reused across distinct signatures"
    );
}
