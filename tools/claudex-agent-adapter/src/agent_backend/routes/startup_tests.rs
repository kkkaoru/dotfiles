use std::sync::{Arc, atomic::Ordering};

use super::super::{AgentBackend, BackendStartup, StartupState};
use super::publish_result_for_generation;

fn alive_backend() -> Arc<AgentBackend> {
    Arc::new(AgentBackend::Pi(
        crate::pi_gateway::PiGateway::alive_for_test(),
    ))
}

fn watch_channel() -> (
    tokio::sync::watch::Sender<StartupState>,
    tokio::sync::watch::Receiver<StartupState>,
) {
    tokio::sync::watch::channel(StartupState::Starting)
}

#[tokio::test]
async fn stale_closed_or_mismatched_generation_reaps_a_successful_backend() {
    let backend = alive_backend();
    let startup = Arc::new(BackendStartup::default());
    startup.closed.store(true, Ordering::Release);
    let (sender, _receiver) = watch_channel();
    publish_result_for_generation(startup, 1, sender, Ok(Arc::clone(&backend))).await;
    assert!(!backend.is_alive());

    let backend = alive_backend();
    let startup = Arc::new(BackendStartup::default());
    startup.generation.store(9, Ordering::Release);
    let (sender, _receiver) = watch_channel();
    publish_result_for_generation(startup, 1, sender, Ok(Arc::clone(&backend))).await;
    assert!(!backend.is_alive());

    let backend = alive_backend();
    let startup = Arc::new(BackendStartup::default());
    startup.generation.store(1, Ordering::Release);
    let (sender, receiver) = watch_channel();
    drop(receiver);
    publish_result_for_generation(startup, 1, sender, Ok(Arc::clone(&backend))).await;
    assert!(!backend.is_alive());
}

#[tokio::test]
async fn stale_closed_startup_discards_a_failed_spawn_result() {
    let startup = Arc::new(BackendStartup::default());
    startup.closed.store(true, Ordering::Release);
    let (sender, _receiver) = watch_channel();
    publish_result_for_generation(startup, 1, sender, Err(Arc::<str>::from("spawn failed"))).await;
}
