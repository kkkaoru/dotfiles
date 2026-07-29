use super::AgentBackend;

impl AgentBackend {
    pub(super) async fn shutdown_leaf(&self) {
        match self {
            Self::Codex(server) => server.shutdown().await,
            Self::Copilot(agent) => agent.shutdown().await,
            Self::ConfiguredAcp(agent) | Self::Grok(agent) => agent.shutdown().await,
            Self::Routed(routes) => Box::pin(routes.shutdown()).await,
        }
    }

    /// Stop every child provider and wait for its direct child process to be
    /// reaped. This is called after the HTTP server has finished draining.
    pub async fn shutdown(&self) {
        match self {
            Self::Routed(routes) => Box::pin(routes.shutdown()).await,
            _ => self.shutdown_leaf().await,
        }
    }
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "lifecycle_tests.rs"]
mod tests;
