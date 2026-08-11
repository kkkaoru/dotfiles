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
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "continuation_tests.rs"]
mod tests;
