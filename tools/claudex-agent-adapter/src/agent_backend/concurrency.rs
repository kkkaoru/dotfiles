use super::AgentBackend;

impl AgentBackend {
    pub(crate) fn max_context_tokens_for_model(&self, model: &str) -> Option<u64> {
        match self {
            Self::Routed(routes) => routes.max_context_tokens_for_model(model),
            Self::SessionScoped(scopes) => scopes.catalog().max_context_tokens_for_model(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => None,
        }
    }

    pub(crate) fn max_concurrency_for_model(&self, model: &str) -> Option<usize> {
        match self {
            Self::Routed(routes) => routes.max_concurrency_for_model(model),
            Self::SessionScoped(scopes) => scopes.catalog().max_concurrency_for_model(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => None,
        }
    }

    pub(crate) fn configured_concurrency_limits(&self) -> Vec<(String, usize)> {
        match self {
            Self::Routed(routes) => routes.configured_concurrency_limits(),
            Self::SessionScoped(scopes) => scopes.catalog().configured_concurrency_limits(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => {
                Vec::new()
            }
        }
    }
}
