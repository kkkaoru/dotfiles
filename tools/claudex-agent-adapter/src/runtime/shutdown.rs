use std::future::Future;

use anyhow::{Context, Result};
use axum::Router;

pub(super) async fn serve(listener: tokio::net::TcpListener, router: Router) -> Result<()> {
    serve_until(listener, router, termination_signal()).await
}

async fn serve_until(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .context("serve adapter HTTP requests")
}

#[cfg(unix)]
async fn termination_signal() {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    signal.recv().await;
    tracing::info!("draining active HTTP requests before shutdown");
}

#[cfg(not(unix))]
async fn termination_signal() {
    std::future::pending().await
}

#[cfg(test)]
include!("shutdown_tests.rs");
