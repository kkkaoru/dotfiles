//! Cancel-and-reuse for outer and same-SubAgent follow-ups while a turn is still streaming.

use std::sync::Arc;

use serde_json::Value;

use super::{
    SelectedSession, Session,
    reservation::{
        find_busy_matching_session, find_busy_signature_matching_session, take_gate_after_preempt,
    },
};
use crate::{
    agent_backend::{AgentBackend, TurnCancellation},
    anthropic::MessagesRequest,
};

pub(super) async fn select_matching_session(
    sessions: Vec<Arc<Session>>,
    request: &MessagesRequest,
    signature: &Arc<str>,
    messages: &[Value],
    app: &AgentBackend,
) -> Option<SelectedSession> {
    if let Some(selected) =
        super::reservation::reserve_matching_session(sessions.clone(), signature, messages).await
    {
        return Some(selected);
    }
    // Follow-ups cancel-and-reuse a busy matching session. Independent parallel
    // SubAgents have distinct `_claudex_transport_identity` signatures, so they
    // do not match. Same-SubAgent TUI follow-ups must preempt so the new user
    // instruction is not stacked behind the in-flight provider turn.
    let is_subagent = crate::anthropic::agent_effort::is_subagent_request(request);
    preempt_busy_matching_session(sessions, request, signature, messages, app, is_subagent).await
}

async fn preempt_busy_matching_session(
    sessions: Vec<Arc<Session>>,
    request: &MessagesRequest,
    signature: &Arc<str>,
    messages: &[Value],
    app: &AgentBackend,
    is_subagent: bool,
) -> Option<SelectedSession> {
    let (session, prior_len) = if is_subagent {
        find_busy_signature_matching_session(sessions, signature, messages).await?
    } else {
        let model = if request.model.is_empty() {
            None
        } else {
            Some(request.model.as_str())
        };
        let user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let claude_session_id = super::super::request_identity::claude_session_id(request);
        find_busy_matching_session(
            sessions,
            signature,
            messages,
            model,
            user_id,
            claude_session_id.as_deref(),
        )
        .await?
    };
    if prior_len == 0 && !is_subagent {
        // An in-flight first turn still has an empty transcript. That prefix
        // matches every new request, so preemption would cancel independent
        // parallel outer turns (Command Code maxConcurrency, multi-tab). Real
        // outer follow-ups arrive after at least one committed transcript entry.
        // SubAgent signatures include agent_id, so an empty transcript still
        // uniquely identifies the same worker follow-up.
        return None;
    }
    tracing::info!(
        thread_id = %session.thread_id,
        prior_transcript_len = prior_len,
        is_subagent,
        "preempting in-flight session for user follow-up"
    );
    let cancellation = app.cancel_turn(&session.thread_id).await;
    if matches!(cancellation, Ok(TurnCancellation::Unsupported)) {
        // Codex app-server cannot interrupt an active turn. Waiting for its
        // gate would defer the new user message for up to three seconds. The
        // request contains the complete transcript, so a fresh provider
        // thread is the lower-latency and lossless fallback.
        tracing::info!(
            thread_id = %session.thread_id,
            "provider cannot cancel active turn; starting a fresh thread for the user follow-up"
        );
        return None;
    }
    let settle_pending = matches!(cancellation, Ok(TurnCancellation::Settled));
    report_cancellation(cancellation, &session.thread_id);
    take_gate_after_preempt(&session, messages, settle_pending).await
}

fn report_cancellation(cancellation: anyhow::Result<TurnCancellation>, thread_id: &str) {
    match cancellation {
        Ok(TurnCancellation::Settled) => {}
        Ok(TurnCancellation::Unsupported) => {
            // Codex cannot interrupt; waiting on the gate still helps once the
            // prior turn finishes naturally (or the client aborted).
            tracing::debug!(
                thread_id = %thread_id,
                "provider cannot cancel turns; waiting for the busy gate"
            );
        }
        Err(error) => {
            tracing::warn!(
                %error,
                thread_id = %thread_id,
                "failed to cancel busy session before follow-up; trying gate wait anyway"
            );
        }
    }
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    include!("preempt_tests.rs");
}
