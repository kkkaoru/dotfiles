use super::{AgentBackend, BackendKind};

impl AgentBackend {
    pub fn backend_kind_for_model(&self, model: &str) -> Option<BackendKind> {
        match self {
            Self::Routed(routes) => routes.backend_kind_for_model(model),
            Self::SessionScoped(scopes) => scopes.catalog().backend_kind_for_model(model),
            Self::Codex(_) => Some(BackendKind::CodexAppServer),
            Self::ConfiguredAcp(_) => Some(BackendKind::ConfiguredAcp),
            Self::Copilot(_) => Some(BackendKind::CopilotAcp),
            Self::Grok(_) => Some(BackendKind::GrokAcp),
        }
    }

    pub fn model_provider_for_model(&self, model: &str) -> Option<String> {
        match self {
            Self::Routed(routes) => routes.model_provider_for_model(model),
            Self::SessionScoped(scopes) => scopes.catalog().model_provider_for_model(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => None,
        }
    }
}
