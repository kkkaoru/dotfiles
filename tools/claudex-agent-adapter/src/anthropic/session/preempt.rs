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
    let (session, prior_len) =
        find_busy_matching_session(sessions, signature, messages, model, user_id).await?;
    tracing::info!(
        thread_id = %session.thread_id,
        prior_transcript_len = prior_len,
        "preempting in-flight session for outer user follow-up"
    );
    report_cancellation(
        app.cancel_turn(&session.thread_id).await,
        &session.thread_id,
    );
    take_gate_after_preempt(&session, messages).await
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
    use std::{
        collections::HashMap,
        os::unix::fs::PermissionsExt,
        sync::Arc,
        time::{Duration, Instant},
    };

    use serde_json::json;
    use tokio::sync::{Mutex, Semaphore};

    use super::*;
    use crate::{agent_backend::AgentBackend, app_server::AppServer};

    #[tokio::test]
    async fn reuses_busy_codex_sessions_after_an_unsupported_cancellation() {
        let app = test_app().await;
        for model in ["main-model", ""] {
            let session = session("main-model", Some("client"));
            let gate = Arc::clone(&session.gate).lock_owned().await;
            let request = request(model);
            let task = start_preemption(Arc::clone(&app), Arc::clone(&session), request);
            tokio::time::sleep(Duration::from_millis(5)).await;
            drop(gate);
            let selected = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .expect("preemption must release the gate")
                .expect("preemption task")
                .expect("reused session");
            assert_eq!(selected.existing_len, 1);
        }
    }

    #[test]
    fn reports_all_preemption_cancellation_outcomes() {
        for cancellation in [
            Ok(TurnCancellation::Settled),
            Ok(TurnCancellation::Unsupported),
            Err(anyhow::anyhow!("provider failure")),
        ] {
            report_cancellation(cancellation, "thread");
        }
    }

    fn start_preemption(
        app: Arc<AgentBackend>,
        session: Arc<Session>,
        request: MessagesRequest,
    ) -> tokio::task::JoinHandle<Option<SelectedSession>> {
        let messages = request.messages.clone();
        tokio::spawn(run_preemption(app, session, request, messages))
    }

    async fn run_preemption(
        app: Arc<AgentBackend>,
        session: Arc<Session>,
        request: MessagesRequest,
        messages: Vec<Value>,
    ) -> Option<SelectedSession> {
        select_matching_session(
            vec![session],
            &request,
            &Arc::from("signature"),
            &messages,
            &app,
        )
        .await
    }

    async fn test_app() -> Arc<AgentBackend> {
        let root = tempfile::tempdir().expect("app-server fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source home");
        std::fs::write(source.join("auth.json"), "{}").expect("auth file");
        let program = root.path().join("codex");
        std::fs::write(
            &program,
            "#!/bin/sh\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
        )
        .expect("mock program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("mock program permissions");
        let server = AppServer::spawn_with_program("main-model", &program, &source, root.path())
            .await
            .expect("mock app server");
        AgentBackend::codex(server)
    }

    fn request(model: &str) -> MessagesRequest {
        MessagesRequest {
            model: model.to_owned(),
            system: json!("system"),
            messages: vec![
                json!({"role":"user","content":"first"}),
                json!({"role":"user","content":"follow-up"}),
            ],
            tools: Vec::new(),
            stream: true,
            output_config: json!({}),
            metadata: json!({"user_id":"client"}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    fn session(model: &str, user_id: Option<&str>) -> Arc<Session> {
        let slots = Arc::new(Semaphore::new(1));
        Arc::new(Session {
            thread_id: "thread".to_owned(),
            model: model.to_owned(),
            disabled_subagent_models: Default::default(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(vec![json!({"role":"user","content":"first"})]),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(Default::default()),
            internal_tools: HashMap::new(),
            external_tool_names: HashMap::new(),
            client_user_id: user_id.map(str::to_owned),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slots.try_acquire_owned().expect("session slot"),
        })
    }
}
