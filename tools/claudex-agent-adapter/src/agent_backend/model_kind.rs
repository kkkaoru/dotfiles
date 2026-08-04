use super::{AgentBackend, BackendKind};

impl AgentBackend {
    pub fn backend_kind_for_model(&self, model: &str) -> Option<BackendKind> {
        match self {
            Self::Routed(routes) => routes.backend_kind_for_model(model),
            Self::Codex(_) => Some(BackendKind::CodexAppServer),
            Self::ConfiguredAcp(_) => Some(BackendKind::ConfiguredAcp),
            Self::Copilot(_) => Some(BackendKind::CopilotAcp),
            Self::Grok(_) => Some(BackendKind::GrokAcp),
        }
    }
}
