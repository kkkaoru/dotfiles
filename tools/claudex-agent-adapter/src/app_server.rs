use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use tokio::process::{Child, ChildStdin};
use tokio::sync::Mutex;

pub(crate) mod events;
use events::ThreadEventDispatcher;
pub use events::ThreadEvents;
mod codex_config;
pub(crate) use codex_config::{
    CODEX_CONFIG_FINGERPRINT_ENV, provider_config_fingerprint, source_home,
};
mod isolated_config;
mod lifecycle;
mod pending;
mod protocol;
mod provider_environment;
mod spawn;
mod rpc;
use pending::PendingResponse;
pub use spawn::response_thread_id;
use spawn::{initialize_params, prepare_isolated_codex_home, spawn_child};

#[cfg(not(coverage_nightly))]
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(coverage_nightly)]
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A persistent JSON-RPC connection to `codex app-server` over JSONL stdio.
pub struct AppServer {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, PendingResponse>>,
    event_dispatcher: ThreadEventDispatcher,
    alive: AtomicBool,
}

impl AppServer {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        let source_home = source_home()?;
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let isolated_home = PathBuf::from(home).join(".cache/claudex/codex-home");
        let program = std::env::var_os("CLAUDEX_CODEX_PROGRAM").unwrap_or_else(|| "codex".into());
        Self::spawn_with_program(model, program, &source_home, &isolated_home).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl AsRef<std::ffi::OsStr>,
        source_home: &std::path::Path,
        isolated_home: &std::path::Path,
    ) -> Result<Arc<Self>> {
        let codex_home = prepare_isolated_codex_home(source_home, isolated_home)?;
        let mut child = spawn_child(model, program, source_home, &codex_home)?;
        let stdin = child
            .stdin
            .take()
            .context("app-server stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout is unavailable")?;
        let server = Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            event_dispatcher: ThreadEventDispatcher::default(),
            alive: AtomicBool::new(true),
        });

        // A weak reader handle lets kill_on_drop stop an abandoned app-server process.
        tokio::spawn(Self::read_loop(Arc::downgrade(&server), stdout));
        let initialize = server
            .request_with_timeout("initialize", initialize_params(), INITIALIZE_TIMEOUT)
            .await
            .context("app-server initialization failed");
        if let Err(error) = initialize {
            server.stop("codex app-server initialization failed").await;
            return Err(error);
        }
        if let Err(error) = server
            .notify("initialized", json!({}))
            .await
            .context("failed to acknowledge app-server initialization")
        {
            server
                .stop("failed to acknowledge codex app-server initialization")
                .await;
            return Err(error);
        }
        Ok(server)
    }
}


#[cfg(test)]
include!("app_server_tests.rs");
