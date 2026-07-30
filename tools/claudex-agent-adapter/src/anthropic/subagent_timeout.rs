use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{
    ActiveTurn, Bridge, MessagesRequest, Segment, Usage, content::anthropic_response,
    model_concurrency::ModelPermit,
};

const DEFAULT_SUBAGENT_RESPONSE_TIMEOUT_SECONDS: u64 = 60;
const SUBAGENT_RESPONSE_TIMEOUT_ENV: &str = "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";
pub(super) const BACKGROUND_NOTICE: &str = "SubAgent is still processing in the background. Do not retry it immediately; continue the task and give the user a concise progress update.";

pub(super) fn subagent_response_timeout() -> Duration {
    subagent_response_timeout_from(|name| std::env::var(name).ok())
}

fn subagent_response_timeout_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    Duration::from_secs(
        get(SUBAGENT_RESPONSE_TIMEOUT_ENV)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_SUBAGENT_RESPONSE_TIMEOUT_SECONDS),
    )
}

pub(super) async fn completes_within<T>(
    timeout: Duration,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::time::timeout(timeout, future).await.ok()
}

impl Bridge {
    pub(super) async fn provider_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        is_subagent: bool,
    ) -> Result<Response<Body>> {
        let concurrency_ticket = self.model_concurrency.ticket(
            &request.model,
            self.app.max_concurrency_for_model(&request.model),
        );
        // Open SSE before prepare_turn so Claude Code receives message_start and
        // keepalives while the provider session starts.
        if request.stream {
            return Ok(self.streaming_messages(
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
            ));
        }
        let permit = match concurrency_ticket {
            Some(ticket) => Some(ticket.acquire().await?),
            None => None,
        };
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        if is_subagent {
            self.non_streaming_subagent_response(turn, permit).await
        } else {
            self.non_streaming_response(turn).await
        }
    }

    pub(super) async fn non_streaming_subagent_response(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) -> Result<Response<Body>> {
        self.non_streaming_subagent_response_with_timeout(turn, permit, subagent_response_timeout())
            .await
    }

    pub(super) async fn non_streaming_subagent_response_with_timeout(
        self: &Arc<Self>,
        mut turn: ActiveTurn,
        permit: Option<ModelPermit>,
        timeout: Duration,
    ) -> Result<Response<Body>> {
        loop {
            let segment = completes_within(
                timeout,
                self.wait_for_segment(
                    &turn.session,
                    &turn.events,
                    turn.input_tokens,
                    &turn.extras,
                    &turn.routing_system,
                    None,
                ),
            )
            .await;
            let Some(segment) = segment else {
                let response = background_response(&turn);
                self.continue_subagent_in_background(turn, permit);
                return Ok(response);
            };
            match segment {
                Ok(segment) => {
                    super::stream::commit_transcript(&turn.session, turn.extras, &segment).await;
                    return Ok(anthropic_response(segment, &turn.response_model));
                }
                Err(error) => {
                    let error_text = error.to_string();
                    let retry = self.context_retry_or_error(&mut turn, error).await?;
                    tracing::warn!(
                        error = %error_text,
                        thread_id = %turn.session.thread_id,
                        "retrying completed SubAgent turn after context window exceeded"
                    );
                    turn = self
                        .retry_after_context_window(retry, &turn.session, turn.input_tokens)
                        .await?;
                }
            }
        }
    }

    pub(super) fn continue_subagent_in_background(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = bridge.non_streaming_response(turn).await {
                tracing::warn!(%error, "background SubAgent turn did not complete");
            }
        });
    }
}

fn background_response(turn: &ActiveTurn) -> Response<Body> {
    anthropic_response(
        Segment {
            blocks: vec![serde_json::json!({"type":"text", "text":BACKGROUND_NOTICE})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens: turn.input_tokens,
                output_tokens: 0,
            },
        },
        &turn.response_model,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        os::unix::fs::PermissionsExt,
        sync::Arc,
        time::Instant,
    };

    use axum::body::to_bytes;
    use serde_json::{Value, json};
    use tokio::sync::{Mutex, Semaphore};

    use super::*;
    use crate::{
        anthropic::{ContextRetry, MessagesRequest, Session},
        app_server::AppServer,
    };

    #[test]
    fn defaults_and_validates_the_subagent_response_timeout() {
        assert_eq!(
            subagent_response_timeout_from(|_| None),
            Duration::from_secs(60)
        );
        assert_eq!(
            subagent_response_timeout_from(|_| Some("7".to_owned())),
            Duration::from_secs(7)
        );
        assert_eq!(
            subagent_response_timeout_from(|_| Some("0".to_owned())),
            Duration::from_secs(60)
        );
        assert_eq!(
            subagent_response_timeout_from(|_| Some("not-a-duration".to_owned())),
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn distinguishes_completed_and_backgrounded_work() {
        assert_eq!(
            completes_within(Duration::from_secs(1), async { 7 }).await,
            Some(7)
        );
        assert_eq!(
            completes_within(Duration::ZERO, std::future::pending::<u8>()).await,
            None
        );
    }

    #[tokio::test]
    async fn returns_a_background_notice_without_losing_the_provider_turn() {
        let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
        let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
        let response = bridge
            .non_streaming_subagent_response_with_timeout(
                active_turn(dispatcher.subscribe("thread"), None).await,
                None,
                Duration::ZERO,
            )
            .await
            .expect("background response");

        assert!(response_text(response).await.contains(BACKGROUND_NOTICE));
    }

    #[tokio::test]
    async fn streams_a_subagent_response_without_waiting_for_the_provider_turn() {
        let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
        let mut request = retry().request;
        request.stream = true;

        let response = bridge
            .provider_messages(request, 1, None, true)
            .await
            .expect("streaming subagent response");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn fails_a_context_window_subagent_turn_without_a_retry() {
        let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
        let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("thread");
        dispatcher.dispatch(json!({
            "method":"error",
            "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
        }));

        let error = bridge
            .non_streaming_subagent_response_with_timeout(
                active_turn(events, None).await,
                None,
                Duration::from_secs(1),
            )
            .await
            .expect_err("unretryable context window error");

        assert!(error.to_string().contains("context window exceeded"));
    }

    #[tokio::test]
    async fn completes_and_retries_a_non_streaming_subagent_turn() {
        let (_root, bridge) = mock_bridge(RETRYING_APP_SERVER).await;
        let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("thread");
        dispatcher.dispatch(json!({
            "method":"error",
            "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
        }));
        let response = bridge
            .non_streaming_subagent_response_with_timeout(
                active_turn(events, Some(retry())).await,
                None,
                Duration::from_secs(1),
            )
            .await
            .expect("retried subagent response");

        let body = response_text(response).await;
        assert!(body.contains("retried"), "unexpected response: {body}");
    }

    async fn mock_bridge(script: &str) -> (tempfile::TempDir, Arc<Bridge>) {
        let root = tempfile::tempdir().expect("subagent timeout fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write source auth");
        let program = root.path().join("mock-app-server");
        std::fs::write(&program, script).expect("write app-server mock");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("make app-server mock executable");
        let app =
            AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
                .await
                .expect("start app-server mock");
        (root, Arc::new(Bridge::new(app, "main".to_owned())))
    }

    async fn active_turn(
        events: crate::app_server::ThreadEvents,
        retry: Option<ContextRetry>,
    ) -> ActiveTurn {
        let slots = Arc::new(Semaphore::new(1));
        let session = Arc::new(Session {
            thread_id: "thread".to_owned(),
            model: "main".to_owned(),
            disabled_subagent_models: Default::default(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            internal_tools: HashMap::new(),
            external_tool_names: HashMap::new(),
            client_user_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slots.try_acquire_owned().expect("session slot"),
        });
        let gate = Arc::clone(&session.gate).lock_owned().await;
        ActiveTurn {
            session,
            events: Arc::new(events),
            response_model: "main".to_owned(),
            extras: Vec::new(),
            routing_system: Value::Null,
            input_tokens: 1,
            retry,
            gate,
        }
    }

    fn retry() -> ContextRetry {
        ContextRetry {
            request: MessagesRequest {
                model: "main".to_owned(),
                system: Value::Null,
                messages: vec![json!({"role":"user","content":"retry"})],
                tools: Vec::new(),
                stream: false,
                output_config: Value::Null,
                metadata: Value::Null,
                working_directory: None,
                disabled_subagent_models: Default::default(),
                claudex_collaborator_model: None,
            },
            effort: None,
            advisor_model: None,
            collaborator_model: None,
        }
    }

    async fn response_text(response: Response<Body>) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        String::from_utf8(bytes.to_vec()).expect("UTF-8 response")
    }

    const STALLED_APP_SERVER: &str = "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n";
    const RETRYING_APP_SERVER: &str = "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread create\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"retried\"}}}'\nread turn\nprintf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"retried\",\"delta\":\"retried\"}}'\nprintf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"retried\",\"turn\":{\"status\":\"completed\"}}}'\nwhile read line; do :; done\n";
}
