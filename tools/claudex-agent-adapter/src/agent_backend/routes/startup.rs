use std::sync::Arc;

use super::{AgentBackend, BackendKind, BackendRoute, BackendStartup, StartupState};

pub(super) fn start_backend(route: BackendRoute) -> tokio::sync::watch::Receiver<StartupState> {
    let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
    tokio::spawn(publish_spawn_result(route, sender));
    receiver
}

pub(super) fn provider_startup(
    kind: BackendKind,
    codex_startup: &Arc<BackendStartup>,
) -> Arc<BackendStartup> {
    match kind {
        BackendKind::CodexAppServer => Arc::clone(codex_startup),
        BackendKind::ConfiguredAcp | BackendKind::CopilotAcp | BackendKind::GrokAcp => {
            Arc::new(BackendStartup::default())
        }
    }
}

async fn publish_spawn_result(
    route: BackendRoute,
    sender: tokio::sync::watch::Sender<StartupState>,
) {
    let result = AgentBackend::spawn_route(&route)
        .await
        .map_err(|error| Arc::<str>::from(format!("{error:#}")));
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
