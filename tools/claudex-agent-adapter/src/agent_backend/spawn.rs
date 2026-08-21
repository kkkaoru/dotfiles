use crate::{app_server::AppServer, pi_gateway::PiGateway};
use anyhow::{Result, bail};
use std::sync::Arc;

use super::{AgentBackend, BackendKind, BackendRoute, RoutedBackends, SessionScopedBackends};

impl AgentBackend {
    pub async fn spawn(kind: BackendKind, model: &str) -> Result<Arc<Self>> {
        match kind {
            BackendKind::CodexAppServer => {
                Ok(Arc::new(Self::Codex(AppServer::spawn(model).await?)))
            }
            BackendKind::PiGateway => {
                bail!("Pi gateway provider and model mapping are required")
            }
        }
    }

    pub(super) async fn spawn_route(route: &BackendRoute) -> Result<Arc<Self>> {
        if route.backend == BackendKind::PiGateway {
            let provider = route
                .pi_provider
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Pi route omitted piProvider"))?;
            let model = route
                .pi_model
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Pi route omitted piModel"))?;
            return Ok(Arc::new(Self::Pi(
                PiGateway::spawn(provider, model, &route.pi_extensions).await?,
            )));
        }
        Self::spawn(route.backend, &route.model).await
    }

    pub fn spawn_routes(routes: &[BackendRoute]) -> Arc<Self> {
        Arc::new(Self::SessionScoped(SessionScopedBackends::new(routes)))
    }

    pub fn codex(server: Arc<AppServer>) -> Arc<Self> {
        Arc::new(Self::Codex(server))
    }

    pub fn pi(gateway: Arc<PiGateway>) -> Arc<Self> {
        Arc::new(Self::Pi(gateway))
    }

    pub fn routed(routes: Vec<(String, Arc<Self>)>) -> Arc<Self> {
        Arc::new(Self::Routed(RoutedBackends::ready(routes)))
    }
}
