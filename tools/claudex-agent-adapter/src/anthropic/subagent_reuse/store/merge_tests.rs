use super::{
    ClaimRecord, LaunchRecord, SessionState, StoredStates, bound_document, merge_session_state,
    prune_persisted_state,
};

fn launch(key: &str, status: &str) -> LaunchRecord {
    LaunchRecord {
        key: key.to_owned(),
        recipient: format!("agent-{key}"),
        scope: "scope".to_owned(),
        model: Some("gpt-test".to_owned()),
        status: status.to_owned(),
    }
}

fn claim(key: &str) -> ClaimRecord {
    ClaimRecord {
        session_id: "session".to_owned(),
        scope: "scope".to_owned(),
        model: Some("gpt-test".to_owned()),
        owner: "owner".to_owned(),
        pid: 1,
        created_revision: 1,
        expires_unix_seconds: 1,
        tool_use_id: key.to_owned(),
    }
}

#[test]
fn merge_keeps_a_terminal_status_when_the_incoming_record_is_still_live() {
    let mut current = SessionState {
        launches: vec![launch("tool-a", "completed")],
    };
    merge_session_state(
        &mut current,
        &SessionState {
            launches: vec![launch("tool-a", "pending")],
        },
    );
    assert_eq!(current.launches[0].status, "completed");
}

#[test]
fn merge_ignores_empty_keys_and_unknown_incoming_records() {
    let mut current = SessionState {
        launches: vec![LaunchRecord {
            key: String::new(),
            recipient: "agent-empty".to_owned(),
            scope: "scope".to_owned(),
            model: None,
            status: "pending".to_owned(),
        }],
    };
    merge_session_state(
        &mut current,
        &SessionState {
            launches: vec![launch("tool-b", "pending")],
        },
    );
    assert_eq!(current.launches.len(), 2);
}

#[test]
fn prune_drops_oldest_launches_past_the_persisted_cap() {
    let mut state = SessionState {
        launches: (0..1_025)
            .map(|index| launch(&format!("tool-{index}"), "pending"))
            .collect(),
    };
    prune_persisted_state(&mut state);
    assert_eq!(state.launches.len(), 1_024);
    assert_eq!(state.launches[0].key, "tool-1");
}

#[test]
fn bound_document_drops_empty_sessions_and_caps_tombstones_and_claims() {
    let mut document = StoredStates {
        version: 2,
        sessions: [
            (String::new(), SessionState::default()),
            (
                "live".to_owned(),
                SessionState {
                    launches: vec![launch("tool-a", "pending")],
                },
            ),
        ]
        .into_iter()
        .collect(),
        session_revisions: [(String::new(), 1), ("live".to_owned(), 2)]
            .into_iter()
            .collect(),
        tombstones: (0..1_025)
            .map(|index| (format!("dead-{index}"), index as u64))
            .collect(),
        claims: (0..4_097)
            .map(|index| (format!("claim-{index}"), claim(&format!("claim-{index}"))))
            .collect(),
        revision: 1,
    };
    document.tombstones.insert("live".to_owned(), 99);
    bound_document(&mut document);
    assert!(!document.sessions.contains_key(""));
    assert!(!document.sessions.contains_key("live"));
    assert!(!document.session_revisions.contains_key(""));
    assert!(!document.tombstones.contains_key(""));
    assert!(document.tombstones.len() <= 1_024);
    assert!(document.claims.len() <= 4_096);
}
