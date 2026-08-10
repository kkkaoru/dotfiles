#[test]
fn claude_session_ids_match_both_some_equal() {
    assert!(super::claude_session_ids_match(Some("sess-1"), Some("sess-1")));
}

#[test]
fn claude_session_ids_match_both_some_different() {
    assert!(!super::claude_session_ids_match(Some("sess-1"), Some("sess-2")));
}

#[test]
fn claude_session_ids_match_both_none() {
    assert!(super::claude_session_ids_match(None, None));
}

#[test]
fn claude_session_ids_match_one_some() {
    assert!(!super::claude_session_ids_match(Some("sess-1"), None));
    assert!(!super::claude_session_ids_match(None, Some("sess-1")));
}

#[test]
fn conversation_matches_all_fields_match() {
    // Create a mock session with specific values
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let session = std::sync::Arc::new(crate::anthropic::Session {
        thread_id: "thread".to_owned(),
        model: "claude-opus-4".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: std::sync::Arc::from("sig"),
        transcript: tokio::sync::Mutex::new(vec![]),
        pending_tools: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        consumed_tool_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        external_tool_names: std::collections::HashMap::new(),
        client_user_id: Some("user-1".to_owned()),
        claude_session_id: Some("claude-sess-1".to_owned()),
        gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });

    assert!(super::conversation_matches(
        &session,
        Some("claude-opus-4"),
        Some("user-1"),
        Some("claude-sess-1")
    ));
}

#[test]
fn conversation_matches_model_mismatch() {
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let session = std::sync::Arc::new(crate::anthropic::Session {
        thread_id: "thread".to_owned(),
        model: "claude-opus-4".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: std::sync::Arc::from("sig"),
        transcript: tokio::sync::Mutex::new(vec![]),
        pending_tools: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        consumed_tool_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        external_tool_names: std::collections::HashMap::new(),
        client_user_id: Some("user-1".to_owned()),
        claude_session_id: Some("claude-sess-1".to_owned()),
        gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });

    assert!(!super::conversation_matches(
        &session,
        Some("gpt-5.3-codex-spark"),
        Some("user-1"),
        Some("claude-sess-1")
    ));
}

#[test]
fn conversation_matches_user_id_mismatch() {
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let session = std::sync::Arc::new(crate::anthropic::Session {
        thread_id: "thread".to_owned(),
        model: "claude-opus-4".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: std::sync::Arc::from("sig"),
        transcript: tokio::sync::Mutex::new(vec![]),
        pending_tools: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        consumed_tool_ids: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        external_tool_names: std::collections::HashMap::new(),
        client_user_id: Some("user-1".to_owned()),
        claude_session_id: Some("claude-sess-1".to_owned()),
        gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });

    assert!(!super::conversation_matches(
        &session,
        Some("claude-opus-4"),
        Some("user-2"),
        Some("claude-sess-1")
    ));
}
