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
    assert!(configured.cancel_turn("session").await.is_err());
    configured.abort_turn_provider("session").await.unwrap();

    let grok = AgentBackend::Grok(crate::grok_acp::GrokAcp::stopped_for_test());
    assert!(grok.cancel_turn("session").await.is_err());
    grok.abort_turn_provider("session").await.unwrap();

    let leaf = Arc::new(AgentBackend::Grok(
        crate::grok_acp::GrokAcp::alive_for_test(),
    ));
    let routed = AgentBackend::routed(vec![("worker".to_owned(), leaf)]);
    assert!(routed.cancel_turn("0:session").await.is_err());
    routed.abort_turn_provider("0:session").await.unwrap();
    assert!(routed.cancel_turn("0:session").await.is_err());
    assert!(routed.abort_turn_provider("0:session").await.is_err());
}
