use std::future::Future;

use anyhow::{Context, Result};
use axum::Router;

pub(super) async fn serve_with_extra_shutdown<L>(
    listener: L,
    router: Router,
    extra: impl Future<Output = ()> + Send + 'static,
) -> Result<()>
where
    L: axum::serve::Listener + Send + 'static,
    L::Addr: std::fmt::Debug + Send + Sync + 'static,
{
    serve_until(listener, router, async {
        tokio::select! {
            () = termination_signal() => {}
            () = extra => {}
        }
    })
    .await
}

async fn serve_until<L>(
    listener: L,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()>
where
    L: axum::serve::Listener + Send + 'static,
    L::Addr: std::fmt::Debug + Send + Sync + 'static,
{
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("serve adapter HTTP requests")
}

#[cfg(unix)]
async fn termination_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("install SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM; draining active HTTP requests before shutdown");
        }
        _ = sigint.recv() => {
            tracing::info!("received SIGINT; draining active HTTP requests before shutdown");
        }
    }
}

#[cfg(not(unix))]
async fn termination_signal() {
    std::future::pending().await
}

#[cfg(test)]
include!("shutdown_tests.rs");
