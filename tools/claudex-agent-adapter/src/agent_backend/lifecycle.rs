use anyhow::{Context, Result};

use super::{AgentBackend, RoutedBackends, TurnCancellation, routed_thread};

async fn cancel_routed_turn(routes: &RoutedBackends, thread_id: &str) -> Result<TurnCancellation> {
    let (index, raw_id) = routed_thread(thread_id);
    let Some(backend) = routes.route(index).ready_backend() else {
        return Ok(TurnCancellation::Settled);
    };
    Box::pin(backend.cancel_turn(raw_id)).await
}

impl AgentBackend {
    pub(crate) async fn cancel_turn(&self, thread_id: &str) -> Result<TurnCancellation> {
        match self {
            Self::Codex(_) => Ok(TurnCancellation::Unsupported),
            Self::Copilot(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::ConfiguredAcp(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::Grok(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::Routed(routes) => cancel_routed_turn(routes, thread_id).await,
        }
    }

    /// Force-close the provider that owns a turn when it has no per-turn
    /// cancellation primitive. Routed providers are retired before shutdown so
    /// a later request starts a fresh leaf instead of reusing the aborted one.
    pub(crate) async fn abort_turn_provider(&self, thread_id: &str) -> Result<()> {
        match self {
            Self::Routed(routes) => {
                let (index, _) = routed_thread(thread_id);
                let route = routes.route(index);
                let backend = route
                    .ready_backend()
                    .context("thread route backend is unavailable during provider abort")?;
                route.retire();
                backend.shutdown_leaf().await;
            }
            _ => self.shutdown_leaf().await,
        }
        Ok(())
    }

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
