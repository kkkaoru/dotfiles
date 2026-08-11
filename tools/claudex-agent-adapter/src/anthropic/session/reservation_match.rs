use std::sync::Arc;

use serde_json::Value;

use super::{
    PREEMPT_GATE_TIMEOUT, SelectedSession, Session, candidate_length, is_better_length,
    touch_session,
};
use crate::anthropic::content::canonical_eq;
use crate::anthropic::content::matching_transcript_len;

pub(super) async fn find_busy_by_signature(
    sessions: impl IntoIterator<Item = Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<(Arc<Session>, usize)> {
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions {
        let Some(existing_len) = candidate_length(&session, signature, messages).await else {
            continue;
        };
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if is_better_length(best.as_ref().map(|(_, len)| *len), existing_len) {
            best = Some((session, existing_len));
        }
    }
    best
}

pub(super) fn conversation_matches(
    session: &Session,
    model: Option<&str>,
    user_id: Option<&str>,
    claude_session_id: Option<&str>,
) -> bool {
    if !claude_session_ids_match(session.claude_session_id.as_deref(), claude_session_id) {
        return false;
    }
    if model.is_some_and(|model| session.model != model) {
        return false;
    }
    match (user_id, session.client_user_id.as_deref()) {
        (Some(left), Some(right)) => left == right,
        // Without a client session id, only allow the fallback when model matches
        // and we have a single busy candidate (checked by caller scoring).
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn claude_session_ids_match(stored: Option<&str>, requested: Option<&str>) -> bool {
    match (stored, requested) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Wait for a cancelled turn to release its session gate, then realign the
/// transcript if a partial assistant message was committed after interrupt.
///
/// When `settle_pending` is true (provider cancel settled), abandon local
/// pending-tool ownership so a pure mid-turn follow-up can reuse the thread.
/// When false, leftover pending tools still block reuse after a failed cancel.
pub(in crate::anthropic::session) async fn take_gate_after_preempt(
    session: &Arc<Session>,
    messages: &[Value],
    settle_pending: bool,
) -> Option<SelectedSession> {
    let gate = tokio::time::timeout(PREEMPT_GATE_TIMEOUT, Arc::clone(&session.gate).lock_owned())
        .await
        .ok()?;
    if has_pending_tools(session).await {
        if !settle_pending {
            return None;
        }
        clear_pending_tools(session).await;
    }
    align_transcript_to_request(session, messages).await;
    let existing_len = matching_transcript_len(session, messages).await?;
    touch_session(session);
    Some(SelectedSession {
        session: Arc::clone(session),
        existing_len,
        recovered: false,
        gate,
    })
}

pub(in crate::anthropic::session) async fn has_pending_tools(session: &Session) -> bool {
    !session.pending_tools.lock().await.is_empty()
}

pub(super) async fn clear_pending_tools(session: &Session) {
    session.pending_tools.lock().await.clear();
    *session
        .pending_since
        .lock()
        .expect("pending tool clock poisoned") = None;
}

/// Drop trailing transcript entries that the client did not keep after interrupt
/// (typically a partial assistant block committed when the prior stream settled).
pub(super) async fn align_transcript_to_request(session: &Session, messages: &[Value]) {
    let mut transcript = session.transcript.lock().await;
    while !transcript_is_prefix(&transcript, messages) {
        // A non-prefix transcript necessarily has at least one entry: an empty
        // transcript is a prefix of every request.
        transcript
            .pop()
            .expect("non-prefix transcript must contain an entry");
    }
}

pub(super) fn transcript_is_prefix(transcript: &[Value], messages: &[Value]) -> bool {
    transcript.len() <= messages.len()
        && transcript
            .iter()
            .zip(messages)
            .all(|(left, right)| canonical_eq(left, right))
}
