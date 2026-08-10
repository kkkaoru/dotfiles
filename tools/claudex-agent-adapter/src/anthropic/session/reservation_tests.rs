use super::*;
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::{Mutex, Semaphore};

fn session(model: &str, client_user_id: Option<&str>) -> Session {
    let slots = Arc::new(Semaphore::new(1));
    Session {
        thread_id: "thread".to_owned(),
        model: model.to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(Default::default()),
        external_tool_names: HashMap::new(),
        client_user_id: client_user_id.map(str::to_owned),
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    }
}

// Lightweight fixtures exercise align/prefix helpers without a full Session.
#[test]
fn transcript_prefix_ignores_cache_control_via_canonical_eq() {
    let left = json!({"role":"user","content":"hi","cache_control":{"type":"ephemeral"}});
    let right = json!({"role":"user","content":"hi"});
    assert!(canonical_eq(&left, &right));
    assert!(transcript_is_prefix(std::slice::from_ref(&left), &[right]));
}

#[test]
fn fallback_identity_checks_model_and_client_user_id() {
    let identified = session("main", Some("client"));
    assert!(conversation_matches(
        &identified,
        Some("main"),
        Some("client"),
        None
    ));
    assert!(conversation_matches(
        &identified,
        None,
        Some("client"),
        None
    ));
    assert!(!conversation_matches(
        &identified,
        Some("other"),
        Some("client"),
        None
    ));
    assert!(!conversation_matches(
        &identified,
        Some("main"),
        Some("other"),
        None
    ));

    let anonymous = session("main", None);
    assert!(conversation_matches(&anonymous, Some("main"), None, None));
    assert!(!conversation_matches(
        &anonymous,
        Some("main"),
        Some("client"),
        None
    ));
}

#[test]
fn fallback_identity_rejects_a_different_claude_session() {
    let mut session = session("main", Some("client"));
    session.claude_session_id = Some("session-a".to_owned());
    assert!(conversation_matches(
        &session,
        Some("main"),
        Some("client"),
        Some("session-a")
    ));
    assert!(!conversation_matches(
        &session,
        Some("main"),
        Some("client"),
        Some("session-b")
    ));
    assert!(!conversation_matches(
        &session,
        Some("main"),
        Some("client"),
        None
    ));
    session.claude_session_id = None;
    assert!(!conversation_matches(
        &session,
        Some("main"),
        Some("client"),
        Some("session-a")
    ));
}

#[tokio::test]
async fn busy_fallback_does_not_reclaim_another_claude_session() {
    let messages = messages();
    let mut other = session("main", Some("client"));
    other.claude_session_id = Some("session-b".to_owned());
    other.signature = Arc::from("other");
    other.transcript = Mutex::new(messages.clone());
    let other = Arc::new(other);
    let _gate = Arc::clone(&other.gate).lock_owned().await;

    assert!(
        find_busy_matching_session(
            vec![Arc::clone(&other)],
            &Arc::from("signature"),
            &messages,
            Some("main"),
            Some("client"),
            Some("session-a"),
        )
        .await
        .is_none(),
        "concurrent claudex TUIs must not share a provider thread"
    );

    assert!(
        find_busy_matching_session(
            vec![other],
            &Arc::from("signature"),
            &messages,
            Some("main"),
            Some("client"),
            Some("session-b"),
        )
        .await
        .is_some(),
        "same Claude session may still reclaim after signature drift"
    );
}

#[tokio::test]
async fn signature_only_busy_match_skips_model_user_fallback() {
    let messages = messages();
    let drifted = session_with("main", Some("client"), "other", messages.clone());
    let _gate = Arc::clone(&drifted.gate).lock_owned().await;
    assert!(
        find_busy_signature_matching_session(
            vec![Arc::clone(&drifted)],
            &Arc::from("signature"),
            &messages,
        )
        .await
        .is_none(),
        "SubAgent preemption must not reclaim another worker via identity fallback"
    );
    assert!(
        find_busy_matching_session(
            vec![drifted],
            &Arc::from("signature"),
            &messages,
            Some("main"),
            Some("client"),
            None,
        )
        .await
        .is_some(),
        "outer preemption still uses identity fallback"
    );
}

#[tokio::test]
async fn signature_only_busy_match_finds_exact_busy_session() {
    let messages = messages();
    let idle = session_with("main", Some("client"), "signature", messages.clone());
    let busy = session_with("main", Some("client"), "signature", messages.clone());
    let other = session_with("main", Some("client"), "other", messages.clone());
    let _busy_gate = Arc::clone(&busy.gate).lock_owned().await;
    let _other_gate = Arc::clone(&other.gate).lock_owned().await;
    let found = find_busy_signature_matching_session(
        vec![idle, Arc::clone(&busy), other],
        &Arc::from("signature"),
        &messages,
    )
    .await
    .expect("exact signature match");
    assert!(Arc::ptr_eq(&found.0, &busy));
    assert_eq!(found.1, messages.len());
}

#[tokio::test]
async fn signature_only_busy_match_accepts_empty_transcript() {
    let messages = messages();
    let busy = session_with("main", Some("client"), "signature", Vec::new());
    let _gate = Arc::clone(&busy.gate).lock_owned().await;
    let found = find_busy_signature_matching_session(
        vec![Arc::clone(&busy)],
        &Arc::from("signature"),
        &messages,
    )
    .await
    .expect("first-turn SubAgent transcript may still be empty");
    assert!(Arc::ptr_eq(&found.0, &busy));
    assert_eq!(found.1, 0);
}

#[tokio::test]
async fn signature_only_busy_match_skips_pending_tools() {
    let messages = messages();
    let session = session_with("main", Some("client"), "signature", messages.clone());
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!(1));
    let _gate = Arc::clone(&session.gate).lock_owned().await;
    assert!(
        find_busy_signature_matching_session(vec![session], &Arc::from("signature"), &messages,)
            .await
            .is_none(),
        "busy SubAgent with pending tools must not be preempted"
    );
}

#[tokio::test]
async fn busy_fallback_rejects_incompatible_candidates() {
    let wrong_model = Arc::new(session("other", Some("client")));
    let anonymous = Arc::new(session("main", None));
    let _wrong_model_gate = Arc::clone(&wrong_model.gate).lock_owned().await;
    let _anonymous_gate = Arc::clone(&anonymous.gate).lock_owned().await;
    let message = json!({"role":"user","content":"follow-up"});

    let found = find_busy_matching_session(
        vec![wrong_model, anonymous],
        &Arc::from("different-signature"),
        &[message],
        Some("main"),
        Some("client"),
        None,
    )
    .await;

    assert!(found.is_none());
}

#[tokio::test]
async fn find_busy_skips_idle_sessions() {
    let gate = Arc::new(Mutex::new(()));
    let _hold = gate.lock().await;
    assert!(gate.clone().try_lock_owned().is_err());
    drop(_hold);
    assert!(gate.try_lock_owned().is_ok());
}

#[tokio::test]
async fn reserve_skips_a_session_with_pending_subagent_tools() {
    let session = Arc::new(session("main", Some("client")));
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!(1));
    let selected = reserve_matching_session(
        vec![session],
        &Arc::from("signature"),
        &[json!({"role":"user","content":"follow-up"})],
    )
    .await;
    assert!(selected.is_none());
}

#[tokio::test]
async fn busy_selection_skips_a_session_with_pending_subagent_tools() {
    let messages = messages();
    let session = session_with("main", Some("client"), "signature", messages.clone());
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!(1));
    let _gate = Arc::clone(&session.gate).lock_owned().await;

    let found = find_busy_matching_session(
        vec![session],
        &Arc::from("signature"),
        &messages,
        Some("main"),
        Some("client"),
        None,
    )
    .await;

    assert!(found.is_none());
}

#[tokio::test]
async fn reserve_reuses_idle_session_when_system_prompt_drifts() {
    let messages = messages();
    let initial = request_like_signature("old system", "Bash", Some("agent-a"));
    let drifted = request_like_signature("new system date stamp", "Bash", Some("agent-a"));
    let session = session_with("auto", Some("client"), &initial, messages.clone());

    let selected =
        reserve_matching_session(vec![Arc::clone(&session)], &Arc::from(drifted), &messages)
            .await
            .expect("system drift must reuse the ACP session");
    assert!(Arc::ptr_eq(&selected.session, &session));
    assert_eq!(selected.existing_len, messages.len());
}

#[tokio::test]
async fn reserve_skips_idle_session_with_a_different_agent_id() {
    let messages = messages();
    let agent_a = request_like_signature("system", "Bash", Some("agent-a"));
    let agent_b = request_like_signature("system", "Bash", Some("agent-b"));
    let session = session_with("auto", Some("client"), &agent_a, messages.clone());

    assert!(
        reserve_matching_session(vec![session], &Arc::from(agent_b), &messages)
            .await
            .is_none(),
        "parallel SubAgents must not share an ACP session"
    );
}

#[tokio::test]
async fn reserve_skips_idle_session_when_tool_names_change() {
    let messages = messages();
    let bash = request_like_signature("system", "Bash", Some("agent-a"));
    let read = request_like_signature("system", "Read", Some("agent-a"));
    let session = session_with("auto", Some("client"), &bash, messages.clone());

    assert!(
        reserve_matching_session(vec![session], &Arc::from(read), &messages)
            .await
            .is_none(),
        "capability changes must still cold-start"
    );
}

#[tokio::test]
async fn reserve_prefers_the_longest_idle_matching_transcript() {
    let messages = messages();
    let busy = session_with("main", Some("client"), "signature", messages.clone());
    let _busy_gate = Arc::clone(&busy.gate).lock_owned().await;
    let wrong_signature = session_with("main", Some("client"), "other", messages.clone());
    let longest = session_with("main", Some("client"), "signature", messages.clone());
    let shortest = session_with("main", Some("client"), "signature", messages[..1].to_vec());

    let selected = reserve_matching_session(
        vec![wrong_signature, busy, Arc::clone(&longest), shortest],
        &Arc::from("signature"),
        &messages,
    )
    .await
    .expect("matching idle session");

    assert!(Arc::ptr_eq(&selected.session, &longest));
    assert_eq!(selected.existing_len, messages.len());
}

#[tokio::test]
async fn busy_selection_skips_idle_sessions_and_keeps_the_longest_match() {
    let messages = messages();
    let idle = session_with("main", Some("client"), "signature", messages.clone());
    let wrong_signature = session_with("main", Some("client"), "other", messages.clone());
    let shortest = session_with("main", Some("client"), "signature", messages[..1].to_vec());
    let longest = session_with("main", Some("client"), "signature", messages.clone());
    let trailing = session_with("main", Some("client"), "signature", messages[..1].to_vec());
    let _wrong_signature_gate = Arc::clone(&wrong_signature.gate).lock_owned().await;
    let _shortest_gate = Arc::clone(&shortest.gate).lock_owned().await;
    let _longest_gate = Arc::clone(&longest.gate).lock_owned().await;
    let _trailing_gate = Arc::clone(&trailing.gate).lock_owned().await;

    let found = find_busy_matching_session(
        vec![
            idle,
            wrong_signature,
            shortest,
            Arc::clone(&longest),
            trailing,
        ],
        &Arc::from("signature"),
        &messages,
        Some("main"),
        Some("client"),
        None,
    )
    .await
    .expect("busy matching session");

    assert!(Arc::ptr_eq(&found.0, &longest));
    assert_eq!(found.1, messages.len());
}

#[tokio::test]
async fn busy_fallback_realigns_a_matching_conversation_after_signature_drift() {
    let messages = messages();
    let wrong_model = session_with("other", Some("client"), "other", messages.clone());
    let realigned = session_with(
        "main",
        Some("client"),
        "other",
        vec![
            messages[0].clone(),
            json!({"role":"assistant","content":"stale"}),
        ],
    );
    let equally_good = session_with("main", Some("client"), "other", messages[..1].to_vec());
    let _wrong_model_gate = Arc::clone(&wrong_model.gate).lock_owned().await;
    let _realigned_gate = Arc::clone(&realigned.gate).lock_owned().await;
    let _equally_good_gate = Arc::clone(&equally_good.gate).lock_owned().await;

    let found = find_busy_matching_session(
        vec![wrong_model, Arc::clone(&realigned), equally_good],
        &Arc::from("signature"),
        &messages,
        Some("main"),
        Some("client"),
        None,
    )
    .await
    .expect("matching busy fallback");

    assert!(Arc::ptr_eq(&found.0, &realigned));
    assert_eq!(found.1, 1);
    assert_eq!(*realigned.transcript.lock().await, messages[..1]);
}
fn messages() -> Vec<Value> {
    vec![
        json!({"role":"user","content":"first"}),
        json!({"role":"user","content":"follow-up"}),
    ]
}

fn request_like_signature(system: &str, tool_name: &str, agent_id: Option<&str>) -> String {
    serde_json::to_string(&json!({
        "system": system,
        "tools": [{
            "name": tool_name,
            "description": "tool schema that Claude Code may rewrite",
            "input_schema": {"type": "object"}
        }],
        "metadata": "client",
        "transport_identity": {
            "session_id": "sess-1",
            "agent_id": agent_id,
            "parent_agent_id": null
        },
        "subagent_spawn_limit_reached": null,
        "working_directory": "/tmp/proj",
        "disabled_subagent_models": [],
        "advisor_model": null,
        "collaborator_model": null
    }))
    .expect("signature json")
}

fn session_with(
    model: &str,
    client_user_id: Option<&str>,
    signature: &str,
    transcript: Vec<Value>,
) -> Arc<Session> {
    let mut session = session(model, client_user_id);
    session.signature = Arc::from(signature);
    session.transcript = Mutex::new(transcript);
    Arc::new(session)
}
