use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{Bridge, Session};

// An abandoned Claude tool request must not reserve a session slot forever.
// Thirty minutes allows long interactive tool work while bounding leaked slots.
const PENDING_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
// A session without pending tools can be reconstructed from Claude Code's next full transcript.
// Thirty minutes covers normal interactive pauses while preventing inactive transcripts from
// occupying session slots indefinitely. Pending and actively-owned sessions are excluded.
const IDLE_SESSION_TTL: Duration = Duration::from_secs(30 * 60);

impl Bridge {
    pub(super) async fn sweep_idle_sessions(&self) {
        let removed = sweep_idle_sessions_at(&self.sessions, Instant::now()).await;
        if removed > 0 {
            tracing::debug!(removed, "released idle claudex sessions");
        }
    }

    pub(super) async fn evict_oldest_idle_session(&self) {
        let Some(session) = take_oldest_evictable_at(&self.sessions, Instant::now()).await else {
            return;
        };
        for (request_id, result) in drain_cancellation_responses(&session).await {
            if let Err(error) = self
                .app
                .respond_for_model(&session.model, request_id, result)
                .await
            {
                tracing::warn!(%error, "failed to cancel an expired Claude tool request");
            }
        }
    }
}

pub(super) async fn sweep_idle_sessions_at(
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

pub(super) async fn record_pending_tool(
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

pub(super) async fn take_oldest_evictable_at(
    sessions: &Mutex<Vec<Arc<Session>>>,
    now: Instant,
) -> Option<Arc<Session>> {
    let mut sessions = sessions.lock().await;
    let mut oldest = None;
    for index in 0..sessions.len() {
        if Arc::strong_count(&sessions[index]) != 1 {
            continue;
        }
        let pending = sessions[index].pending_tools.lock().await;
        let expired = !pending.is_empty() && pending_expired(&sessions[index], now);
        if pending.is_empty() || expired {
            drop(pending);
            let activity = session_activity(&sessions[index]);
            if oldest.is_none_or(|(_, oldest_activity)| activity < oldest_activity) {
                oldest = Some((index, activity));
            }
        }
    }
    oldest.map(|(index, _)| sessions.remove(index))
}

pub(super) async fn drain_cancellation_responses(session: &Session) -> Vec<(Value, Value)> {
    let responses = session
        .pending_tools
        .lock()
        .await
        .drain()
        .map(|(_, id)| (id, cancellation_result()))
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

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{collections::HashMap, os::unix::fs::PermissionsExt, path::Path};

    use serde_json::json;
    use tokio::sync::Semaphore;

    use super::*;
    use crate::app_server::AppServer;

    #[tokio::test]
    async fn bridge_evicts_expired_tools_and_sends_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let app = spawn_app(root.path(), true).await;
        let bridge = Bridge::new(app, "model".to_owned());
        let expired = Instant::now() - PENDING_SESSION_TTL;
        bridge.sessions.lock().await.push(session(expired));

        bridge.evict_oldest_idle_session().await;

        assert!(bridge.sessions.lock().await.is_empty());
        bridge.evict_oldest_idle_session().await;
    }

    #[tokio::test]
    async fn bridge_sweeps_an_expired_idle_session_during_request_maintenance() {
        let root = tempfile::tempdir().unwrap();
        let app = spawn_app(root.path(), true).await;
        let bridge = Bridge::new(app, "model".to_owned());
        let idle = session(Instant::now() - IDLE_SESSION_TTL);
        idle.pending_tools.lock().await.clear();
        *idle.pending_since.lock().unwrap() = None;
        bridge.sessions.lock().await.push(idle);

        bridge.sweep_idle_sessions().await;
        bridge.sweep_idle_sessions().await;

        assert!(bridge.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn eviction_tolerates_a_closed_app_server() {
        let root = tempfile::tempdir().unwrap();
        let app = spawn_app(root.path(), false).await;
        let bridge = Bridge::new(app, "model".to_owned());
        let expired = Instant::now() - PENDING_SESSION_TTL;
        bridge.sessions.lock().await.push(session(expired));
        wait_until_stopped(&bridge).await;

        bridge.evict_oldest_idle_session().await;

        assert!(bridge.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn retains_a_newer_candidate_seen_after_the_oldest() {
        let now = Instant::now();
        let oldest_activity = now - IDLE_SESSION_TTL - Duration::from_secs(2);
        let oldest = session(oldest_activity);
        oldest.pending_tools.lock().await.clear();
        let newer = session(now - IDLE_SESSION_TTL - Duration::from_secs(1));
        newer.pending_tools.lock().await.clear();
        let sessions = Mutex::new(vec![Arc::clone(&oldest), newer]);
        drop(oldest);

        let evicted = take_oldest_evictable_at(&sessions, now).await.unwrap();

        assert_eq!(*evicted.last_activity.lock().unwrap(), oldest_activity);
    }

    fn session(activity: Instant) -> Arc<Session> {
        let slots = Arc::new(Semaphore::new(1));
        Arc::new(Session {
            thread_id: "thread".to_owned(),
            model: "main-model".to_owned(),
            signature: "signature".to_owned(),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::from([("tool".to_owned(), json!(9))])),
            consumed_tool_ids: Mutex::new(Default::default()),
            internal_tools: HashMap::new(),
            external_tool_names: HashMap::new(),
            client_user_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(activity),
            pending_since: std::sync::Mutex::new(Some(activity)),
            _slot: slots.try_acquire_owned().unwrap(),
        })
    }

    async fn spawn_app(root: &Path, keep_open: bool) -> Arc<AppServer> {
        let source = root.join("source");
        let isolated = root.join("isolated");
        let program = root.join("app-server");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("auth.json"), "{}").unwrap();
        let tail = if keep_open {
            "while read line; do :; done"
        } else {
            "exit 0"
        };
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nread line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread line\n{tail}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        AppServer::spawn_with_program("model", program, &source, &isolated)
            .await
            .unwrap()
    }

    async fn wait_until_stopped(bridge: &Bridge) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while bridge.app.is_alive() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fixture app-server closes");
    }
}
