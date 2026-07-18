use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::body::to_bytes;
use serde_json::{Value, json};

use super::{
    MessagesRequest, Segment, Session, Usage,
    content::*,
    retention::{record_pending_tool, sweep_idle_sessions_at, take_oldest_evictable_at},
    session::{codex_tool_name, dynamic_tool, internal_advisor_tool, internal_collaborator_tool},
    stream::send_stream_frame,
    trace_request,
};

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
        signature: "signature".to_owned(),
        transcript: tokio::sync::Mutex::new(Vec::new()),
        pending_tools: tokio::sync::Mutex::new(pending_tools),
        consumed_tool_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        internal_tools: HashMap::new(),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(tokio::sync::Mutex::new(())),
        last_activity: std::sync::Mutex::new(last_activity),
        pending_since: std::sync::Mutex::new(has_pending_tool.then_some(last_activity)),
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
    let expired = test_session_at(&semaphore, false, now - Duration::from_secs(31 * 60));
    let active = test_session_at(&semaphore, false, now - Duration::from_secs(31 * 60));
    let active_owner = Arc::clone(&active);
    let pending = test_session_at(&semaphore, true, now - Duration::from_secs(60 * 60));
    let fresh_activity = now - Duration::from_secs(29 * 60);
    let fresh = test_session_at(&semaphore, false, fresh_activity);
    let sessions = tokio::sync::Mutex::new(vec![expired, active, pending, fresh]);

    assert_eq!(sweep_idle_sessions_at(&sessions, now).await, 1);

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

    assert_eq!(internal_advisor_tool()["name"], "advisor");
    assert_eq!(internal_collaborator_tool()["name"], "claude_collaborator");
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
            },
        },
        "model",
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response["content"][0]["text"], "OK");
    assert_eq!(response["usage"]["input_tokens"], 10);
    assert_eq!(response["usage"]["output_tokens"], 2);

    let error = error_response(
        axum::http::StatusCode::BAD_REQUEST,
        anyhow::anyhow!("bad request"),
    );
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = to_bytes(error.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("bad request"));
}

#[test]
fn extracts_signatures_and_counts() {
    let request: MessagesRequest = serde_json::from_value(json!({
        "system":"system",
        "messages":[{"role":"user","content":"hello"}],
        "tools":[]
    }))
    .unwrap();
    assert!(
        request_signature(&request, Some("test-advisor"), Some("test-collaborator"))
            .unwrap()
            .contains("test-advisor")
    );
    let serialized_bytes = serde_json::to_string(&request.system).unwrap().len()
        + serde_json::to_string(&request.messages).unwrap().len()
        + serde_json::to_string(&request.tools).unwrap().len();
    assert_eq!(token_count(&request), serialized_bytes.div_ceil(4));
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
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::with_default(subscriber, || trace_request(&request));
}
