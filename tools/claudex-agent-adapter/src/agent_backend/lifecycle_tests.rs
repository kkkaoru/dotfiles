use std::sync::Arc;

use super::super::{AgentBackend, routes::RoutedBackends};

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
