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
    // Shared provider children must survive a single target's failed
    // cancellation. ACP cancellation is session-targeted; killing this leaf
    // would also terminate clean sibling sessions sharing the persistent
    // configured/Grok/Copilot child. Codex has the same invariant because its
    // app-server owns the shared prompt cache. The target turn is already
    // invalidated by the ACP cancellation path, so leave the child alive for
    // unrelated turns and future reuse.
    // Every backend reachable through `RoutedBackends` is a provider leaf:
    // `RoutedBackend::ready` requires `backend.kind()`, and `spawn_route`
    // constructs only Codex/ACP/Copilot/Grok leaves. A routed/session-scoped
    // backend cannot be installed as a route without violating that contract.
    debug_assert!(matches!(
        backend.as_ref(),
        AgentBackend::Codex(_)
            | AgentBackend::ConfiguredAcp(_)
            | AgentBackend::Copilot(_)
            | AgentBackend::Grok(_)
            | AgentBackend::Pi(_)
    ));
    tracing::debug!(
        thread_id,
        model = %route.model,
        "retaining shared provider after target-specific abort"
    );
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
            Self::Pi(gateway) => {
                gateway.cancel_turn(thread_id)?;
                Ok(TurnCancellation::Settled)
            }
            Self::Routed(routes) => cancel_routed_turn(routes, thread_id).await,
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().cancel_turn(thread_id)).await
            }
        }
    }

    /// Recover a turn whose provider has no settled per-turn cancellation
    /// primitive. Routed ACP/Codex pools retain their child because the abort
    /// identifies one target session, not every clean sibling using that
    /// persistent child. Standalone leaves still shut down below.
    pub(crate) async fn abort_turn_provider(&self, thread_id: &str) -> Result<()> {
        match self {
            // A routed ACP pool can retain the provider above so a failed
            // target cancellation cannot take down a clean sibling. A leaf
            // provider has no sibling ownership context, so shut it down.
            Self::Codex(_) => {}
            Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) | Self::Pi(_) => {
                self.shutdown_leaf().await
            }
            Self::Routed(routes) => abort_routed_provider(routes, thread_id).await?,
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().abort_turn_provider(thread_id)).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn shutdown_leaf(&self) {
        match self {
            Self::Codex(server) => server.shutdown().await,
            Self::Copilot(agent) => agent.shutdown().await,
            Self::ConfiguredAcp(agent) | Self::Grok(agent) => agent.shutdown().await,
            Self::Pi(gateway) => gateway.shutdown().await,
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
