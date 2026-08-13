use std::sync::Arc;

use super::{AgentBackend, BackendRoute, BackendStartup, StartupState};

pub(super) fn start_backend(
    route: BackendRoute,
    startup: Arc<BackendStartup>,
    generation: u64,
) -> tokio::sync::watch::Receiver<StartupState> {
    let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
    tokio::spawn(publish_spawn_result(route, startup, generation, sender));
    receiver
}

async fn publish_spawn_result(
    route: BackendRoute,
    startup: Arc<BackendStartup>,
    generation: u64,
    sender: tokio::sync::watch::Sender<StartupState>,
) {
    let result = AgentBackend::spawn_route(&route)
        .await
        .map_err(|error| Arc::<str>::from(format!("{error:#}")));
    publish_result_for_generation(startup, generation, sender, result).await;
}

async fn publish_result_for_generation(
    startup: Arc<BackendStartup>,
    generation: u64,
    sender: tokio::sync::watch::Sender<StartupState>,
    result: Result<Arc<AgentBackend>, Arc<str>>,
) {
    if startup.closed.load(std::sync::atomic::Ordering::Acquire)
        || startup
            .generation
            .load(std::sync::atomic::Ordering::Acquire)
            != generation
        || sender.is_closed()
    {
        if let Ok(backend) = result {
            // A stale spawn must never become reachable after route retire;
            // close the child here even when the waiting caller was dropped.
            backend.shutdown().await;
        }
        return;
    }
    publish_result(sender, result).await;
}

pub(super) async fn publish_result(
    sender: tokio::sync::watch::Sender<StartupState>,
    result: Result<Arc<AgentBackend>, Arc<str>>,
) {
    if sender.is_closed() {
        if let Ok(backend) = result {
            backend.shutdown().await;
        }
        return;
    }
    sender.send_replace(StartupState::Ready(result));
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "startup_tests.rs"]
mod startup_tests;
