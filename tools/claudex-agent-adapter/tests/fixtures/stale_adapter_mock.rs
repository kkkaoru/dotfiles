use std::{
    fs::File,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, routing::get};
use claudex_agent_adapter::agent_backend::{BackendKind, BackendRoute};
use serde_json::json;

const RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(5);
const STALE_BUILD_ID: &str = "protocol-compatible-stale-build";
const TEST_MODEL: &str = "test-main-model";
const TEST_MAX_PROCESSES: usize = 20;
const TEST_TIMEOUT_MINUTES: u64 = 120;

#[derive(Clone)]
struct SlowState {
    entered: Arc<PathBuf>,
    release: Arc<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = std::env::args().collect::<Vec<_>>();
    anyhow::ensure!(
        arguments.get(1).map(String::as_str) == Some("serve"),
        "expected serve"
    );
    let listen = option(&arguments, "--listen")?
        .parse::<SocketAddr>()
        .context("parse listen address")?;
    let state = SlowState {
        entered: Arc::new(PathBuf::from(option(&arguments, "--entered")?)),
        release: Arc::new(PathBuf::from(option(&arguments, "--release")?)),
    };
    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/slow", get(slow))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .context("bind stale adapter")?;
    axum::serve(listener, router)
        .with_graceful_shutdown(termination_signal())
        .await
        .context("serve stale adapter")
}

fn option<'a>(arguments: &'a [String], name: &str) -> Result<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .with_context(|| format!("missing {name}"))
}

async fn health() -> Json<serde_json::Value> {
    let route = BackendRoute::new(TEST_MODEL, BackendKind::CodexAppServer).description();
    Json(json!({
        "status": "ok",
        "pid": std::process::id(),
        "protocol_version": claudex_agent_adapter::ADAPTER_PROTOCOL_VERSION,
        "build_id": STALE_BUILD_ID,
        "backend_routes": [route],
        "subscription_max_processes": TEST_MAX_PROCESSES,
        "subscription_timeout_minutes": TEST_TIMEOUT_MINUTES
    }))
}

async fn models() -> Json<serde_json::Value> {
    Json(json!({"data": []}))
}

async fn slow(State(state): State<SlowState>) -> &'static str {
    publish_file(state.entered.as_ref(), b"entered");
    while !state.release.exists() {
        tokio::time::sleep(RELEASE_POLL_INTERVAL).await;
    }
    "complete"
}

fn publish_file(path: &Path, contents: &[u8]) {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = File::create(&tmp).expect("entered tmp");
    file.write_all(contents).expect("entered bytes");
    file.sync_all().expect("entered fsync");
    drop(file);
    std::fs::rename(tmp, path).expect("publish entered");
}

#[cfg(unix)]
async fn termination_signal() {
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    signal.recv().await;
}

#[cfg(not(unix))]
async fn termination_signal() {
    std::future::pending().await
}
