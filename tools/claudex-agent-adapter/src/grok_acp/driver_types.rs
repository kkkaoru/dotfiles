use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::oneshot;

use super::connection::AcpProvider;
use crate::app_server::events::ThreadEventDispatcher;

pub(super) enum DriverCommand {
    CreateSession {
        params: Value,
        _permit: tokio::sync::OwnedSemaphorePermit,
        response: oneshot::Sender<Result<Value>>,
    },
    StartTurn {
        params: Value,
        permit: tokio::sync::OwnedSemaphorePermit,
        response: oneshot::Sender<Result<()>>,
    },
    CancelTurn {
        session_id: String,
        response: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        response: oneshot::Sender<()>,
    },
}

pub(super) struct DriverSetup {
    pub(super) provider: AcpProvider,
    pub(super) program: OsString,
    pub(super) arguments: Option<Vec<String>>,
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) cwd: PathBuf,
    pub(super) events: Arc<ThreadEventDispatcher>,
    pub(super) alive: Arc<AtomicBool>,
    pub(super) cooldown: Arc<AtomicBool>,
    pub(super) ready: oneshot::Sender<Result<()>>,
}

pub(super) struct DriverThread {
    handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    joined: tokio::sync::watch::Sender<bool>,
}

fn finish_driver_thread(
    handle: std::thread::JoinHandle<()>,
    joined: tokio::sync::watch::Sender<bool>,
) -> std::thread::Result<()> {
    let result = handle.join();
    joined.send_replace(true);
    result
}

async fn join_driver_thread(
    handle: std::thread::JoinHandle<()>,
    joined: tokio::sync::watch::Sender<bool>,
) {
    let join = tokio::task::spawn_blocking(move || finish_driver_thread(handle, joined));
    if let Err(error) = join.await {
        tracing::error!(?error, "failed to join ACP driver thread");
    }
}

impl DriverThread {
    pub(super) fn new(handle: std::thread::JoinHandle<()>) -> Self {
        let (joined, _) = tokio::sync::watch::channel(false);
        Self {
            handle: std::sync::Mutex::new(Some(handle)),
            joined,
        }
    }

    pub(super) async fn join(&self) {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            join_driver_thread(handle, self.joined.clone()).await;
            return;
        }

        if *self.joined.borrow() {
            return;
        }
        let mut joined = self.joined.subscribe();
        if !*joined.borrow_and_update() {
            let _ = joined.changed().await;
        }
    }

    #[cfg(test)]
    pub(super) fn completed() -> Self {
        let (joined, _) = tokio::sync::watch::channel(true);
        Self {
            handle: std::sync::Mutex::new(None),
            joined,
        }
    }

    #[cfg(test)]
    pub(super) fn is_joined(&self) -> bool {
        *self.joined.borrow()
    }
}
