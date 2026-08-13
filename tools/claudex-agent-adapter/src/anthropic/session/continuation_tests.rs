use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use super::*;
use crate::agent_backend::AgentBackend;

fn request(messages: Vec<Value>) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages,
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: json!({"user_id": "outer"}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn session(model: &str, user_id: Option<&str>, transcript: Vec<Value>) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: model.to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(transcript),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        // A live provider session keeps at least one recovered native
        // capability even when the continuation omits the schemas.
        // Stale/no-tool resume behavior is covered separately in the
        // session tests with an explicitly empty map.
        external_tool_names: HashMap::from([("cc_Bash_0".to_owned(), "Bash".to_owned())]),
        client_user_id: user_id.map(str::to_owned),
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    })
}

#[tokio::test]
async fn skips_ineligible_candidates_and_keeps_the_longest_matching_session() {
    let messages = vec![
        json!({"role":"user","content":"first"}),
        json!({"role":"assistant","content":"second"}),
        json!({"role":"user","content":"third"}),
    ];
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let wrong_model = session("other", Some("outer"), messages[..2].to_vec());
    let wrong_user = session("main", Some("another"), messages[..2].to_vec());
    let busy = session("main", Some("outer"), messages[..2].to_vec());
    let pending = session("main", Some("outer"), messages[..2].to_vec());
    pending
        .pending_tools
        .lock()
        .await
        .insert("toolu_pending".to_owned(), Value::Null);
    let mismatched = session(
        "main",
        Some("outer"),
        vec![json!({"role":"user","content":"different"})],
    );
    let longest = session("main", Some("outer"), messages.clone());
    let shorter = session("main", Some("outer"), messages[..2].to_vec());
    let busy_gate = Arc::clone(&busy.gate).lock_owned().await;
    bridge.sessions.lock().await.extend([
        wrong_model,
        wrong_user,
        busy,
        pending,
        mismatched,
        Arc::clone(&longest),
        shorter,
    ]);

    let selected = select_toolless_main_session(&bridge, &request(messages))
        .await
        .expect("the longest idle matching session is selected");

    assert!(Arc::ptr_eq(&selected.session, &longest));
    assert_eq!(selected.existing_len, 3);
    drop(selected);
    drop(busy_gate);
}

#[tokio::test]
async fn toolless_continuation_skips_another_claude_session() {
    let messages = vec![json!({"role":"user","content":"first"})];
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut other = session("main", Some("outer"), messages.clone());
    Arc::get_mut(&mut other)
        .expect("unique session")
        .claude_session_id = Some("session-b".to_owned());
    bridge.sessions.lock().await.push(other);

    let mut request = request(messages);
    request.metadata = json!({
        "user_id":"outer",
        "_claudex_transport_identity":{"session_id":"session-a"}
    });
    assert!(
        select_toolless_main_session(&bridge, &request)
            .await
            .is_none(),
        "toolless continuation must not attach another claudex TUI"
    );
}

#[tokio::test]
async fn toolless_continuation_reuses_matching_claude_session_ids() {
    let messages = vec![json!({"role":"user","content":"first"})];
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut matching = session("main", Some("outer"), messages.clone());
    Arc::get_mut(&mut matching)
        .expect("unique session")
        .claude_session_id = Some("session-a".to_owned());
    bridge.sessions.lock().await.push(Arc::clone(&matching));

    let mut request = request(messages);
    request.metadata = json!({
        "user_id":"outer",
        "_claudex_transport_identity":{"session_id":"session-a"}
    });
    let selected = select_toolless_main_session(&bridge, &request)
        .await
        .expect("matching Claude session id may continue");
    assert!(Arc::ptr_eq(&selected.session, &matching));
}
