//! Cancel-and-reuse for outer user follow-ups while a turn is still streaming.

use std::sync::Arc;

use serde_json::Value;

use super::{
    SelectedSession, Session,
    reservation::{find_busy_matching_session, take_gate_after_preempt},
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
    // Outer (non-SubAgent) follow-ups cancel-and-reuse the busy main session
    // instead of cold-starting a second provider thread. Parallel SubAgents
    // keep the skip-busy fork so they do not preempt each other.
    if crate::anthropic::agent_effort::is_subagent_request(request) {
        return None;
    }
    preempt_busy_matching_session(sessions, request, signature, messages, app).await
}

async fn preempt_busy_matching_session(
    sessions: Vec<Arc<Session>>,
    request: &MessagesRequest,
    signature: &Arc<str>,
    messages: &[Value],
    app: &AgentBackend,
) -> Option<SelectedSession> {
    let model = if request.model.is_empty() {
        None
    } else {
        Some(request.model.as_str())
    };
    let user_id = request.metadata.get("user_id").and_then(Value::as_str);
    let (session, prior_len) = find_busy_matching_session(
        sessions,
        signature,
        messages,
        model,
        user_id,
    )
    .await?;
    tracing::info!(
        thread_id = %session.thread_id,
        prior_transcript_len = prior_len,
        "preempting in-flight session for outer user follow-up"
    );
    match app.cancel_turn(&session.thread_id).await {
        Ok(TurnCancellation::Settled) => {}
        Ok(TurnCancellation::Unsupported) => {
            // Codex cannot interrupt; waiting on the gate still helps once the
            // prior turn finishes naturally (or the client aborted).
            tracing::debug!(
                thread_id = %session.thread_id,
                "provider cannot cancel turns; waiting for the busy gate"
            );
        }
        Err(error) => {
            tracing::warn!(
                %error,
                thread_id = %session.thread_id,
                "failed to cancel busy session before follow-up; trying gate wait anyway"
            );
        }
    }
    take_gate_after_preempt(&session, messages).await
}
