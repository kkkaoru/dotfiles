pub use crate::web_search::WebSearchMode;
use crate::{
    app_server::{AppServer, ThreadEvents},
    copilot_acp::CopilotAcp,
    grok_acp::GrokAcp,
};
use anyhow::Result;
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
mod session_scope;
mod spawn;
use request::{routed_thread, subscribe_routed_thread};
use routes::{RoutedBackend, RoutedBackends};
pub(crate) use session_scope::{ProviderSessionScopeSnapshot, SessionScopedBackends};
#[path = "agent_backend_route.rs"]
mod route;
pub use route::{AcpLaunch, BackendRoute};

pub enum AgentBackend {
    Codex(Arc<AppServer>),
    Copilot(Arc<CopilotAcp>),
    ConfiguredAcp(Arc<GrokAcp>),
    Grok(Arc<GrokAcp>),
    Routed(RoutedBackends),
    /// Claude-session-keyed pools of [`RoutedBackends`].
    SessionScoped(SessionScopedBackends),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TurnCancellation {
    Settled,
    Unsupported,
}
impl AgentBackend {
    pub const fn kind(&self) -> BackendKind {
        match self {
            Self::Codex(_) => BackendKind::CodexAppServer,
            Self::ConfiguredAcp(_) => BackendKind::ConfiguredAcp,
            Self::Copilot(_) => BackendKind::CopilotAcp,
            Self::Grok(_) => BackendKind::GrokAcp,
            Self::Routed(_) | Self::SessionScoped(_) => {
                panic!("a routed backend has no single kind")
            }
        }
    }

    /// Resolve the Claude-session provider pool. Non-scoped backends return self.
    pub fn scope_or_self(self: &Arc<Self>, claude_session_id: Option<&str>) -> Arc<Self> {
        match self.as_ref() {
            Self::SessionScoped(scopes) => scopes.scope(claude_session_id),
            _ => Arc::clone(self),
        }
    }

    pub async fn release_session_scope(&self, claude_session_id: Option<&str>) {
        if let Self::SessionScoped(scopes) = self {
            scopes.release_scope(claude_session_id).await;
        }
    }

    pub fn supports_model(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.supports(model),
            Self::SessionScoped(scopes) => scopes.catalog().supports(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => false,
        }
    }
    pub fn web_search_mode(&self, model: &str) -> WebSearchMode {
        match self {
            Self::Routed(routes) => routes.web_search_mode(model),
            Self::SessionScoped(scopes) => scopes.catalog().web_search_mode(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => {
                WebSearchMode::default()
            }
        }
    }

    pub(crate) fn launch_scoped_effort(&self, model: &str) -> Option<String> {
        match self {
            Self::Routed(routes) => routes.launch_scoped_effort(model),
            Self::SessionScoped(scopes) => scopes.catalog().launch_scoped_effort(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => None,
        }
    }
    pub fn route_descriptions(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.descriptions(),
            Self::SessionScoped(scopes) => scopes.catalog().descriptions(),
            leaf => vec![leaf.kind().to_string()],
        }
    }
    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.models(),
            Self::SessionScoped(scopes) => scopes.catalog().models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.started_models(),
            Self::SessionScoped(scopes) => scopes.started_models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub(crate) fn provider_session_scope_count(&self) -> usize {
        match self {
            Self::SessionScoped(scopes) => scopes.scope_count(),
            _ => 0,
        }
    }

    pub(crate) fn provider_session_scopes(
        &self,
    ) -> Vec<crate::agent_backend::ProviderSessionScopeSnapshot> {
        match self {
            Self::SessionScoped(scopes) => scopes.scope_snapshots(),
            _ => Vec::new(),
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
            Self::SessionScoped(scopes) => scopes.scope(None).subscribe_thread(thread_id),
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
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.scope(None).ensure_thread_ready(thread_id)).await
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
            Self::SessionScoped(_) => true,
        }
    }

    pub(crate) fn model_is_alive(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.model_is_alive(model),
            Self::SessionScoped(scopes) => scopes.model_is_alive(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => {
                self.is_alive()
            }
        }
    }
}

#[cfg(test)]
include!("agent_backend_tests.rs");
