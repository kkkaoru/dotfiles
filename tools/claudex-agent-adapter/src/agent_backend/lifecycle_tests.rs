use super::super::{AgentBackend, routes::RoutedBackends};

#[tokio::test]
async fn leaf_shutdown_returns_for_a_routed_backend() {
    let backend = AgentBackend::Routed(RoutedBackends::lazy(&[]));

    backend.shutdown_leaf().await;

    let backend = AgentBackend::Routed(RoutedBackends::lazy(&[]));
    backend.shutdown().await;
}
