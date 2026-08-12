use anyhow::{Context, Result};

use super::{AgentBackend, RoutedBackends, TurnCancellation, routed_thread};

async fn cancel_routed_turn(routes: &RoutedBackends, thread_id: &str) -> Result<TurnCancellation> {
    let (index, raw_id) = routed_thread(thread_id);
    let Some(backend) = routes.route(index).ready_backend() else {
        return Ok(TurnCancellation::Settled);
    };
    Box::pin(backend.cancel_turn(raw_id)).await
}

async fn abort_routed_provider(routes: &RoutedBackends, thread_id: &str) -> Result<()> {
    let (index, _) = routed_thread(thread_id);
    let route = routes.route(index);
    let backend = route
        .ready_backend()
        .context("thread route backend is unavailable during provider abort")?;
    // Shared Codex app-server must survive a single disconnect — retiring the
    // route clears startup for every Codex model and forces a cold respawn.
    if matches!(backend.as_ref(), AgentBackend::Codex(_)) {
        return Ok(());
    }
    route.retire();
    backend.shutdown_leaf().await;
    Ok(())
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
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().cancel_turn(thread_id)).await
            }
        }
    }

    /// Force-close the provider that owns a turn when it has no per-turn
    /// cancellation primitive. Routed non-Codex providers are retired before
    /// shutdown so a later request starts a fresh leaf. Codex app-server is
    /// shared across routes/threads — aborting must not kill it or every
    /// idle prompt-cache / SubAgent reuse slot dies with the one disconnect.
    pub(crate) async fn abort_turn_provider(&self, thread_id: &str) -> Result<()> {
        match self {
            Self::Codex(_) => {}
            Self::Routed(routes) => abort_routed_provider(routes, thread_id).await?,
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().abort_turn_provider(thread_id)).await?;
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
            Self::SessionScoped(scopes) => Box::pin(scopes.shutdown_all()).await,
        }
    }

    /// Stop every child provider and wait for its direct child process to be
    /// reaped. This is called after the HTTP server has finished draining.
    pub async fn shutdown(&self) {
        match self {
            Self::Routed(routes) => Box::pin(routes.shutdown()).await,
            Self::SessionScoped(scopes) => Box::pin(scopes.shutdown_all()).await,
            _ => self.shutdown_leaf().await,
        }
    }
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "lifecycle_tests.rs"]
mod tests;
