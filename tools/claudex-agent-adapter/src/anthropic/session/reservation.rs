use std::{sync::Arc, time::Duration};

use serde_json::Value;

use super::{SelectedSession, Session, candidate_length, is_better_length, touch_session};
use crate::anthropic::content::matching_transcript_len;

/// How long a follow-up may wait for an in-flight same-session turn to release
/// its gate after cancellation. Kept short so cold create_session remains an
/// option when the prior turn cannot settle.
pub(super) const PREEMPT_GATE_TIMEOUT: Duration = Duration::from_millis(500);

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
        // Idle sessions that still own Claude tool_use blocks are waiting for
        // tool_result (or TaskStop/TaskOutput control). Reclaiming them here
        // would settle those launches and break parallel-agent follow-ups.
        // Busy/preempt paths may still abandon pending tools after a settled
        // cancel so a true mid-turn interrupt can continue on the same thread.
        if has_pending_tools(&session).await {
            continue;
        }
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

/// Busy SubAgent follow-ups must match `_claudex_transport_identity` exactly.
/// Model+user fallback would cancel an unrelated parallel worker.
pub(super) async fn find_busy_signature_matching_session(
    sessions: Vec<Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<(Arc<Session>, usize)> {
    find_busy_by_signature(sessions, signature, messages).await
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
    claude_session_id: Option<&str>,
) -> Option<(Arc<Session>, usize)> {
    if let Some(best) = find_busy_by_signature(sessions.iter().cloned(), signature, messages).await
    {
        return Some(best);
    }
    // Signature miss: still reclaim a busy conversation for the same human
    // *and* the same Claude Code session. Model+user alone would mix SubAgents
    // across concurrent claudex TUIs on one daemon.
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions {
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if !conversation_matches(&session, model, user_id, claude_session_id) {
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

#[path = "reservation_match.rs"]
mod reservation_match;
#[cfg(test)]
use crate::anthropic::content::canonical_eq;
use reservation_match::{
    align_transcript_to_request, conversation_matches, find_busy_by_signature,
};
#[cfg(test)]
use reservation_match::{claude_session_ids_match, transcript_is_prefix};
pub(super) use reservation_match::{has_pending_tools, take_gate_after_preempt};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "reservation_tests.rs"]
mod tests;
