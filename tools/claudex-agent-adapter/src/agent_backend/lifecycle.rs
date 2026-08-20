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
    // cancellation. Codex app-server owns the shared prompt cache, and Pi
    // gateway sessions are request-scoped. Leave the child alive for
    // unrelated turns and future reuse.
    debug_assert!(matches!(
        backend.as_ref(),
        AgentBackend::Codex(_) | AgentBackend::Pi(_)
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
    /// primitive. Routed Codex/Pi pools retain their child because the abort
    /// identifies one target session, not every clean sibling using that
    /// persistent child. Standalone leaves still shut down below.
    pub(crate) async fn abort_turn_provider(&self, thread_id: &str) -> Result<()> {
        match self {
            Self::Codex(_) => {}
            Self::Pi(_) => self.shutdown_leaf().await,
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "lifecycle_tests.rs"]
mod tests;
