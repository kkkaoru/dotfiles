use std::{sync::Arc, time::Instant};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::super::{Bridge, Session};
use super::{IDLE_SESSION_TTL, PENDING_SESSION_TTL, SESSION_SWEEP_INTERVAL};
use crate::anthropic::content::pending_request_id;

pub(super) fn session_sweep_due(clock: &std::sync::Mutex<Instant>, now: Instant) -> bool {
    let mut next = clock.lock().expect("session sweep clock poisoned");
    if now < *next {
        return false;
    }
    *next = now + SESSION_SWEEP_INTERVAL;
    true
}

pub(super) async fn respond_to_evicted_request(
    bridge: &Bridge,
    model: &str,
    request_id: Value,
    result: Value,
) {
    if let Err(error) = bridge
        .app
        .respond_for_model(model, request_id, result)
        .await
    {
        tracing::warn!(%error, "failed to cancel an expired Claude tool request");
    }
}

pub(crate) async fn sweep_idle_sessions_at(
    sessions: &Mutex<Vec<Arc<Session>>>,
    now: Instant,
) -> usize {
    let mut sessions = sessions.lock().await;
    let before = sessions.len();
    let mut index = 0;
    while index < sessions.len() {
        if Arc::strong_count(&sessions[index]) != 1 {
            index += 1;
            continue;
        }
        let pending = sessions[index].pending_tools.lock().await;
        let idle = pending.is_empty()
            && now.saturating_duration_since(session_activity(&sessions[index]))
                >= IDLE_SESSION_TTL;
        drop(pending);
        if idle {
            sessions.remove(index);
        } else {
            index += 1;
        }
    }
    before - sessions.len()
}

pub(crate) async fn record_pending_tool(
    session: &Session,
    tool_use_id: String,
    request_id: Value,
    emitted_at: Instant,
) {
    session
        .pending_tools
        .lock()
        .await
        .insert(tool_use_id, request_id);
    *session
        .pending_since
        .lock()
        .expect("pending tool clock poisoned") = Some(emitted_at);
    *session
        .last_activity
        .lock()
        .expect("session clock poisoned") = emitted_at;
}

pub(crate) async fn take_oldest_evictable_at(
    sessions: &Mutex<Vec<Arc<Session>>>,
    now: Instant,
) -> Option<Arc<Session>> {
    let mut sessions = sessions.lock().await;
    let mut oldest = None;
    for index in 0..sessions.len() {
        let Some(activity) = evictable_activity(&sessions[index], now).await else {
            continue;
        };
        if oldest.is_none_or(|(_, oldest_activity)| activity < oldest_activity) {
            oldest = Some((index, activity));
        }
    }
    oldest.map(|(index, _)| sessions.remove(index))
}

async fn evictable_activity(session: &Arc<Session>, now: Instant) -> Option<Instant> {
    if Arc::strong_count(session) != 1 {
        return None;
    }
    let pending = session.pending_tools.lock().await;
    let expired = !pending.is_empty() && pending_expired(session, now);
    (pending.is_empty() || expired).then(|| session_activity(session))
}

pub(super) async fn drain_cancellation_responses(session: &Session) -> Vec<(Value, Value)> {
    let responses = session
        .pending_tools
        .lock()
        .await
        .drain()
        .map(|(_, id)| (pending_request_id(&id), cancellation_result()))
        .collect();
    *session
        .pending_since
        .lock()
        .expect("pending tool clock poisoned") = None;
    responses
}

fn cancellation_result() -> Value {
    json!({
        "contentItems":[{
            "type":"inputText",
            "text":"Claude Code did not return this tool result before the session expired."
        }],
        "success":false
    })
}

fn session_activity(session: &Session) -> Instant {
    *session
        .last_activity
        .lock()
        .expect("session clock poisoned")
}

fn pending_expired(session: &Session, now: Instant) -> bool {
    session
        .pending_since
        .lock()
        .expect("pending tool clock poisoned")
        .is_some_and(|since| now.saturating_duration_since(since) >= PENDING_SESSION_TTL)
}
