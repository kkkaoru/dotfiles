//! Reuse an outer thread when Claude Code omits unchanged tool schemas.

use std::sync::Arc;

use super::{Bridge, MessagesRequest, SelectedSession, Session, is_better_length, touch_session};
use crate::anthropic::content::matching_transcript_len;

/// Keep dynamic Claude Code tools when an outer continuation omits the
/// otherwise unchanged schema list. A non-empty tool list still requires an
/// exact signature match, allowing real capability changes to start a fresh
/// provider thread.
pub(super) async fn select_toolless_main_session(
    bridge: &Bridge,
    request: &MessagesRequest,
) -> Option<SelectedSession> {
    let model = bridge.request_model(request);
    let client_user_id = request
        .metadata
        .get("user_id")
        .and_then(serde_json::Value::as_str);
    let claude_session_id = super::super::request_identity::claude_session_id(request);
    let sessions = bridge.sessions.lock().await.clone();
    let mut best: Option<SelectedSession> = None;
    for session in sessions {
        if !same_main_conversation(
            &session,
            &model,
            client_user_id,
            claude_session_id.as_deref(),
        ) {
            continue;
        }
        let Ok(gate) = Arc::clone(&session.gate).try_lock_owned() else {
            continue;
        };
        if !session.pending_tools.lock().await.is_empty() {
            continue;
        }
        let Some(existing_len) = matching_transcript_len(&session, &request.messages).await else {
            continue;
        };
        if is_better_length(
            best.as_ref().map(|selected| selected.existing_len),
            existing_len,
        ) {
            best = Some(SelectedSession {
                session,
                existing_len,
                recovered: false,
                gate,
            });
        }
    }
    if let Some(selected) = &best {
        touch_session(&selected.session);
    }
    best
}

fn same_main_conversation(
    session: &Session,
    model: &str,
    client_user_id: Option<&str>,
    claude_session_id: Option<&str>,
) -> bool {
    match (session.claude_session_id.as_deref(), claude_session_id) {
        (Some(left), Some(right)) if left != right => return false,
        (None, None) => {}
        (Some(_), Some(_)) => {}
        _ => return false,
    }
    session.model == model && session.client_user_id.as_deref() == client_user_id
}

#[cfg(test)]
// Test fixtures are intentionally excluded from production coverage accounting.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
}
