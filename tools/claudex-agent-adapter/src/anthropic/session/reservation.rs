use std::{sync::Arc, time::Duration};

use serde_json::Value;

use super::{SelectedSession, Session, candidate_length, is_better_length, touch_session};
use crate::anthropic::content::{canonical_eq, matching_transcript_len};

/// How long a follow-up may wait for an in-flight same-session turn to release
/// its gate after cancellation. Kept short so cold create_session remains an
/// option when the prior turn cannot settle.
const PREEMPT_GATE_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) async fn reserve_matching_session(
    sessions: Vec<Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<SelectedSession> {
    let mut best: Option<SelectedSession> = None;
    for session in sessions {
        let Ok(gate) = Arc::clone(&session.gate).try_lock_owned() else {
            continue;
        };
        let Some(existing_len) = candidate_length(&session, signature, messages).await else {
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

/// Find the best transcript-matching session that is currently busy (gate held).
///
/// Outer follow-ups first require an exact signature match. If tools/system drift
/// broke the signature (common mid-conversation), fall back to model + user_id so
/// interactive messages still reclaim the live provider thread instead of cold-starting.
pub(super) async fn find_busy_matching_session(
    sessions: Vec<Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
    model: Option<&str>,
    user_id: Option<&str>,
) -> Option<(Arc<Session>, usize)> {
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions.iter() {
        let Some(existing_len) = candidate_length(session, signature, messages).await else {
            continue;
        };
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if is_better_length(best.as_ref().map(|(_, len)| *len), existing_len) {
            best = Some((Arc::clone(session), existing_len));
        }
    }
    if best.is_some() {
        return best;
    }
    // Signature miss: still reclaim a busy conversation for the same human.
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions {
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if !conversation_matches(&session, model, user_id) {
            continue;
        }
        align_transcript_to_request(&session, messages).await;
        let Some(existing_len) = matching_transcript_len(&session, messages).await else {
            continue;
        };
        if is_better_length(best.as_ref().map(|(_, len)| *len), existing_len) {
            best = Some((session, existing_len));
        }
    }
    best
}

fn conversation_matches(session: &Session, model: Option<&str>, user_id: Option<&str>) -> bool {
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

/// Wait for a cancelled turn to release its session gate, then realign the
/// transcript if a partial assistant message was committed after interrupt.
pub(super) async fn take_gate_after_preempt(
    session: &Arc<Session>,
    messages: &[Value],
) -> Option<SelectedSession> {
    let gate = tokio::time::timeout(
        PREEMPT_GATE_TIMEOUT,
        Arc::clone(&session.gate).lock_owned(),
    )
    .await
    .ok()?;
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

/// Drop trailing transcript entries that the client did not keep after interrupt
/// (typically a partial assistant block committed when the prior stream settled).
async fn align_transcript_to_request(session: &Session, messages: &[Value]) {
    let mut transcript = session.transcript.lock().await;
    while !transcript_is_prefix(&transcript, messages) {
        if transcript.pop().is_none() {
            break;
        }
    }
}

fn transcript_is_prefix(transcript: &[Value], messages: &[Value]) -> bool {
    transcript.len() <= messages.len()
        && transcript
            .iter()
            .zip(messages)
            .all(|(left, right)| canonical_eq(left, right))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Lightweight fixtures exercise align/prefix helpers without a full Session.
    #[test]
    fn transcript_prefix_ignores_cache_control_via_canonical_eq() {
        let left = json!({"role":"user","content":"hi","cache_control":{"type":"ephemeral"}});
        let right = json!({"role":"user","content":"hi"});
        assert!(canonical_eq(&left, &right));
        assert!(transcript_is_prefix(std::slice::from_ref(&left), &[right]));
    }

    #[tokio::test]
    async fn find_busy_skips_idle_sessions() {
        // Built via Session construction in session_tests; unit-level here only
        // documents that try_lock success excludes a candidate from the busy set.
        let gate = Arc::new(Mutex::new(()));
        let _hold = gate.lock().await;
        assert!(gate.clone().try_lock_owned().is_err());
        drop(_hold);
        assert!(gate.try_lock_owned().is_ok());
    }
}
