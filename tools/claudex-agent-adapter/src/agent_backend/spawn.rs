use crate::{
    app_server::AppServer, copilot_acp::CopilotAcp, grok_acp::GrokAcp, pi_gateway::PiGateway,
};
use anyhow::{Result, bail};
use std::sync::Arc;

use super::{AgentBackend, BackendKind, BackendRoute, RoutedBackends, SessionScopedBackends};

impl AgentBackend {
    pub async fn spawn(kind: BackendKind, model: &str) -> Result<Arc<Self>> {
        match kind {
            BackendKind::CodexAppServer => {
                Ok(Arc::new(Self::Codex(AppServer::spawn(model).await?)))
            }
            BackendKind::ConfiguredAcp => bail!("configured ACP launch details are required"),
            BackendKind::CopilotAcp => Ok(Arc::new(Self::Copilot(CopilotAcp::spawn(model).await?))),
            BackendKind::GrokAcp => Ok(Arc::new(Self::Grok(GrokAcp::spawn(model).await?))),
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
        if let Some(acp) = &route.acp {
            let agent = GrokAcp::spawn_configured_with_max_concurrency(
                &route.model,
                acp,
                route.max_concurrency,
                route.effort.as_deref(),
            )
            .await?;
            return Ok(Arc::new(Self::ConfiguredAcp(agent)));
        }
        if route.backend == BackendKind::GrokAcp {
            return Ok(Arc::new(Self::Grok(
                GrokAcp::spawn_with_effort(
                    &route.model,
                    route
                        .effort
                        .as_deref()
                        .unwrap_or(crate::grok_acp::DEFAULT_REASONING_EFFORT),
                )
                .await?,
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

    pub fn grok(agent: Arc<GrokAcp>) -> Arc<Self> {
        Arc::new(Self::Grok(agent))
    }

    pub fn copilot(agent: Arc<CopilotAcp>) -> Arc<Self> {
        Arc::new(Self::Copilot(agent))
    }

    pub fn configured_acp(agent: Arc<GrokAcp>) -> Arc<Self> {
        Arc::new(Self::ConfiguredAcp(agent))
    }

    pub fn routed(routes: Vec<(String, Arc<Self>)>) -> Arc<Self> {
        Arc::new(Self::Routed(RoutedBackends::ready(routes)))
    }
}
