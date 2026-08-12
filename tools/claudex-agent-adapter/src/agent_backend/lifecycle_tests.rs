use std::sync::Arc;

use super::super::{AgentBackend, routes::RoutedBackends};

#[cfg(unix)]
#[tokio::test]
async fn codex_abort_turn_provider_keeps_shared_app_server_alive() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let root = tempfile::tempdir().expect("codex abort fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let program = root.path().join("codex-abort-noop");
    std::fs::write(
        &program,
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nwhile read line; do :; done\n",
    )
    .expect("write fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fixture");
    let server = crate::app_server::AppServer::spawn_with_program(
        "model",
        &program,
        &source,
        &root.path().join("codex-abort-home"),
    )
    .await
    .expect("start codex fixture");
    assert!(server.is_alive());
    let backend = AgentBackend::Codex(Arc::clone(&server));
    backend
        .abort_turn_provider("thread-1")
        .await
        .expect("Codex abort is a no-op");
    assert!(
        server.is_alive(),
        "Codex abort must not kill the shared app-server (prompt-cache / SubAgent reuse)"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn leaf_shutdown_returns_for_a_routed_backend() {
    let backend = AgentBackend::Routed(RoutedBackends::lazy(&[]));

    backend.shutdown_leaf().await;

    let backend = AgentBackend::Routed(RoutedBackends::lazy(&[]));
    backend.shutdown().await;
}

#[tokio::test]
async fn cancellation_and_abort_cover_each_leaf_and_routed_route() {
    let copilot = AgentBackend::Copilot(crate::copilot_acp::CopilotAcp::settled_for_test().await);
    assert_eq!(
        copilot.cancel_turn("session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    copilot.abort_turn_provider("session").await.unwrap();

    let configured = AgentBackend::ConfiguredAcp(crate::grok_acp::GrokAcp::stopped_for_test());
    assert_eq!(
        configured.cancel_turn("session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    configured.abort_turn_provider("session").await.unwrap();

    let grok = AgentBackend::Grok(crate::grok_acp::GrokAcp::stopped_for_test());
    assert_eq!(
        grok.cancel_turn("session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    grok.abort_turn_provider("session").await.unwrap();

    let leaf = Arc::new(AgentBackend::Grok(
        crate::grok_acp::GrokAcp::alive_for_test(),
    ));
    let routed = AgentBackend::routed(vec![("worker".to_owned(), leaf)]);
    assert_eq!(
        routed.cancel_turn("0:session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    routed.abort_turn_provider("0:session").await.unwrap();
    // A concurrent event subscriber must observe a closed stream, not panic,
    // after the routed leaf has been retired by the abort path.
    let events = routed.subscribe_thread("0:session");
    assert!(events.recv().await.is_none());
    assert_eq!(
        routed.cancel_turn("0:session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    assert!(routed.abort_turn_provider("0:session").await.is_err());
}

#[tokio::test]
async fn session_scoped_respond_for_model_uses_the_pool_that_started_the_model() {
    use crate::agent_backend::{BackendKind, BackendRoute};

    let scoped = AgentBackend::spawn_routes(&[BackendRoute::new(
        "glm-5.2:cloud",
        BackendKind::CodexAppServer,
    )]);
    let AgentBackend::SessionScoped(scopes) = scoped.as_ref() else {
        panic!("expected SessionScoped backends");
    };
    let leaf = Arc::new(AgentBackend::Grok(
        crate::grok_acp::GrokAcp::alive_for_test(),
    ));
    scopes.insert_scope_for_test(
        "tui-session",
        AgentBackend::routed(vec![("glm-5.2:cloud".to_owned(), leaf)]),
    );
    let error = scoped
        .respond_for_model("glm-5.2:cloud", serde_json::json!(1), serde_json::json!({}))
        .await
        .expect_err("Grok leaf rejects Claude tool results");
    let message = error.to_string();
    assert!(
        !message.contains("not initialized"),
        "tool result must not hit the uninitialized anonymous pool: {message}"
    );
    assert!(
        message.contains("Grok ACP"),
        "expected the started Claude-session pool: {message}"
    );
}

#[tokio::test]
async fn session_scoped_cancel_abort_and_shutdown_reach_inner_routes() {
    use crate::agent_backend::{BackendKind, BackendRoute};

    let scoped =
        AgentBackend::spawn_routes(&[BackendRoute::new("worker", BackendKind::ConfiguredAcp)]);
    assert_eq!(
        scoped.cancel_turn("0:session").await.unwrap(),
        super::super::TurnCancellation::Settled
    );
    // Configured ACP is lazy — abort before the leaf is ready surfaces unavailable.
    assert!(scoped.abort_turn_provider("0:session").await.is_err());
    scoped.shutdown_leaf().await;
    scoped.shutdown().await;
}

#[tokio::test]
async fn leaf_backends_report_kind_and_omit_model_provider() {
    let copilot = AgentBackend::Copilot(crate::copilot_acp::CopilotAcp::settled_for_test().await);
    assert_eq!(
        copilot.backend_kind_for_model("any"),
        Some(super::super::BackendKind::CopilotAcp)
    );
    assert_eq!(copilot.model_provider_for_model("any"), None);

    let configured = AgentBackend::ConfiguredAcp(crate::grok_acp::GrokAcp::stopped_for_test());
    assert_eq!(
        configured.backend_kind_for_model("any"),
        Some(super::super::BackendKind::ConfiguredAcp)
    );
    assert_eq!(configured.model_provider_for_model("any"), None);

    let grok = AgentBackend::Grok(crate::grok_acp::GrokAcp::stopped_for_test());
    assert_eq!(
        grok.backend_kind_for_model("any"),
        Some(super::super::BackendKind::GrokAcp)
    );
    assert_eq!(grok.model_provider_for_model("any"), None);
}
