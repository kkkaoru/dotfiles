pub use crate::web_search::WebSearchMode;
use crate::{
    app_server::{AppServer, ThreadEvents},
    copilot_acp::CopilotAcp,
    grok_acp::GrokAcp,
};
use anyhow::{Result, bail};
use std::sync::Arc;
mod kind;
mod model_kind;
pub use kind::BackendKind;
mod concurrency;
mod dispatch;
mod lifecycle;
mod request;
mod route_config;
mod routes;
use request::{routed_thread, subscribe_routed_thread};
use routes::{RoutedBackend, RoutedBackends};
#[path = "agent_backend_route.rs"]
mod route;
pub use route::{AcpLaunch, BackendRoute};

pub enum AgentBackend {
    Codex(Arc<AppServer>),
    Copilot(Arc<CopilotAcp>),
    ConfiguredAcp(Arc<GrokAcp>),
    Grok(Arc<GrokAcp>),
    Routed(RoutedBackends),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TurnCancellation {
    Settled,
    Unsupported,
}
impl AgentBackend {
    pub async fn spawn(kind: BackendKind, model: &str) -> Result<Arc<Self>> {
        match kind {
            BackendKind::CodexAppServer => {
                Ok(Arc::new(Self::Codex(AppServer::spawn(model).await?)))
            }
            BackendKind::ConfiguredAcp => bail!("configured ACP launch details are required"),
            BackendKind::CopilotAcp => Ok(Arc::new(Self::Copilot(CopilotAcp::spawn(model).await?))),
            BackendKind::GrokAcp => Ok(Arc::new(Self::Grok(GrokAcp::spawn(model).await?))),
        }
    }
    async fn spawn_route(route: &BackendRoute) -> Result<Arc<Self>> {
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
        Arc::new(Self::Routed(RoutedBackends::lazy(routes)))
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
    pub const fn kind(&self) -> BackendKind {
        match self {
            Self::Codex(_) => BackendKind::CodexAppServer,
            Self::ConfiguredAcp(_) => BackendKind::ConfiguredAcp,
            Self::Copilot(_) => BackendKind::CopilotAcp,
            Self::Grok(_) => BackendKind::GrokAcp,
            Self::Routed(_) => panic!("a routed backend has no single kind"),
        }
    }
    pub fn supports_model(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.supports(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => false,
        }
    }
    pub fn web_search_mode(&self, model: &str) -> WebSearchMode {
        match self {
            Self::Routed(routes) => routes.web_search_mode(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => {
                WebSearchMode::default()
            }
        }
    }

    pub(crate) fn launch_scoped_effort(&self, model: &str) -> Option<String> {
        match self {
            Self::Routed(routes) => routes.launch_scoped_effort(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => None,
        }
    }
    pub fn route_descriptions(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.descriptions(),
            leaf => vec![leaf.kind().to_string()],
        }
    }
    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.started_models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        match self {
            Self::Codex(server) => server.subscribe_thread(thread_id),
            Self::Copilot(agent) => agent.subscribe_thread(thread_id),
            Self::ConfiguredAcp(agent) => agent.subscribe_thread(thread_id),
            Self::Grok(agent) => agent.subscribe_thread(thread_id),
            Self::Routed(routes) => {
                let (index, raw_id) = routed_thread(thread_id);
                subscribe_routed_thread(routes.route(index).as_ref(), thread_id, raw_id)
            }
        }
    }

    /// Wait for the leaf provider that owns a routed thread to be available.
    /// Route startup is lazy and can race with a turn's event subscription.
    /// Production callers should await this before subscribing; the
    /// synchronous `subscribe_thread` fallback still protects against a
    /// concurrent retire/abort that happens after this wait.
    pub async fn ensure_thread_ready(&self, thread_id: &str) -> Result<()> {
        match self {
            Self::Routed(routes) => {
                let (index, _) = routed_thread(thread_id);
                routes.route(index).get().await.map(|_| ())
            }
            Self::Codex(_) | Self::Copilot(_) | Self::ConfiguredAcp(_) | Self::Grok(_) => Ok(()),
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Self::Codex(server) => server.is_alive(),
            Self::Copilot(agent) => agent.is_alive(),
            Self::ConfiguredAcp(agent) => agent.is_alive(),
            Self::Grok(agent) => agent.is_alive(),
            Self::Routed(routes) => routes.is_alive(),
        }
    }

    pub(crate) fn model_is_alive(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.model_is_alive(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => {
                self.is_alive()
            }
        }
    }
}

#[cfg(test)]
include!("agent_backend_tests.rs");
