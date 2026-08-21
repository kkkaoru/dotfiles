use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use anyhow::{Result, bail};

use super::{AgentBackend, BackendKind, BackendRoute, WebSearchMode};

mod backends;
mod concurrency;
mod query;
mod resolve;
mod shutdown;
mod startup;

pub use backends::RoutedBackends;
use startup::start_backend;

pub(super) const MAX_DYNAMIC_ROUTES: usize = 32;

pub(super) struct RoutedBackend {
    pub(super) model: String,
    pub(super) kind: BackendKind,
    template: BackendRoute,
    startup: Arc<BackendStartup>,
    activated: AtomicBool,
}

#[derive(Default)]
pub(super) struct BackendStartup {
    receiver: Mutex<Option<tokio::sync::watch::Receiver<StartupState>>>,
    /// Monotonically increasing startup generation. Retiring a route fences
    /// every in-flight spawn that still owns an old watch sender, including a
    /// caller that is waiting on a cloned receiver.
    generation: AtomicU64,
    /// Permanent pool teardown fence. A retired route may restart, while a
    /// session pool that was shut down must never resurrect a provider for a
    /// late caller holding an old receiver.
    closed: AtomicBool,
}

#[derive(Clone)]
pub(super) enum StartupState {
    Starting,
    Ready(Result<Arc<AgentBackend>, Arc<str>>),
}

impl RoutedBackend {
    pub(super) fn lazy(route: BackendRoute, startup: Arc<BackendStartup>) -> Self {
        Self {
            model: route.model.clone(),
            kind: route.backend,
            template: route,
            startup,
            activated: AtomicBool::new(false),
        }
    }

    pub(super) fn ready(model: String, backend: Arc<AgentBackend>) -> Self {
        let kind = backend.kind();
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        sender.send_replace(StartupState::Ready(Ok(backend)));
        let startup = Arc::new(BackendStartup::default());
        *startup.receiver.lock().expect("backend startup poisoned") = Some(receiver);
        Self {
            template: BackendRoute::new(&model, kind),
            model,
            kind,
            startup,
            activated: AtomicBool::new(true),
        }
    }

    pub(super) async fn get(&self) -> Result<Arc<AgentBackend>> {
        self.activated.store(true, Ordering::Relaxed);
        if self.startup.closed.load(Ordering::Acquire) {
            bail!("backend route pool is shut down");
        }
        if let Some(backend) = self.ready_backend() {
            return Ok(backend);
        }
        let (generation, startup) = self.startup_receiver()?;
        wait_for_started_backend(self, generation, startup).await
    }

    async fn advance_startup_state(
        &self,
        generation: &mut u64,
        startup: &mut tokio::sync::watch::Receiver<StartupState>,
        state: StartupState,
    ) -> Result<Option<Arc<AgentBackend>>> {
        match state {
            StartupState::Starting => {
                self.advance_closed_startup(generation, startup).await?;
                Ok(None)
            }
            StartupState::Ready(Ok(backend))
                if self.startup_generation() == *generation && backend.is_alive() =>
            {
                Ok(Some(backend))
            }
            StartupState::Ready(Ok(backend)) => {
                (*generation, *startup) = self.retry_stale_backend(*generation, backend).await?;
                Ok(None)
            }
            StartupState::Ready(Err(error)) => bail!(error.to_string()),
        }
    }

    /// Advance past a `Starting` observation, adopting a fresh startup
    /// generation if the current one closed without ever becoming ready.
    async fn advance_closed_startup(
        &self,
        generation: &mut u64,
        startup: &mut tokio::sync::watch::Receiver<StartupState>,
    ) -> Result<()> {
        if let Some((next_generation, next_startup)) =
            self.retry_closed_startup(*generation, startup).await?
        {
            *generation = next_generation;
            *startup = next_startup;
        }
        Ok(())
    }

    async fn retry_closed_startup(
        &self,
        generation: u64,
        startup: &mut tokio::sync::watch::Receiver<StartupState>,
    ) -> Result<Option<(u64, tokio::sync::watch::Receiver<StartupState>)>> {
        if startup.changed().await.is_ok() {
            return Ok(None);
        }
        // `retire` deliberately closes the receiver while a caller may still
        // be waiting on it. Retry against a fresh generation; a genuinely
        // crashed startup is still reported instead of spinning forever.
        if self.startup_generation() == generation {
            bail!("backend startup task stopped without a result");
        }
        Ok(Some(self.startup_receiver()?))
    }

    async fn retry_stale_backend(
        &self,
        generation: u64,
        backend: Arc<AgentBackend>,
    ) -> Result<(u64, tokio::sync::watch::Receiver<StartupState>)> {
        // A route can be retired while its provider process is being created.
        // Never hand that stale process to the caller; it belongs to the
        // retired generation.
        backend.shutdown().await;
        let next = self.startup_receiver()?;
        if next.0 == generation {
            bail!("backend startup generation became stale without a new generation");
        }
        Ok(next)
    }

    fn startup_generation(&self) -> u64 {
        self.startup.generation.load(Ordering::Acquire)
    }

    fn startup_receiver(&self) -> Result<(u64, tokio::sync::watch::Receiver<StartupState>)> {
        if self.startup.closed.load(Ordering::Acquire) {
            bail!("backend route pool is shut down");
        }
        let mut startup = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned");
        if self.startup.closed.load(Ordering::Acquire) {
            bail!("backend route pool is shut down");
        }
        let reusable = startup
            .as_ref()
            .is_some_and(|receiver| match receiver.borrow().clone() {
                StartupState::Starting => {
                    let receiver = receiver.clone();
                    !receiver.has_changed().is_err()
                }
                StartupState::Ready(Ok(backend)) => backend.is_alive(),
                StartupState::Ready(Err(_)) => false,
            });
        let generation = if !reusable {
            let generation = self.startup.generation.fetch_add(1, Ordering::AcqRel) + 1;
            *startup = Some(start_backend(
                self.template.clone(),
                Arc::clone(&self.startup),
                generation,
            ));
            generation
        } else {
            self.startup_generation()
        };
        Ok((
            generation,
            startup.as_ref().expect("backend startup receiver").clone(),
        ))
    }

    pub(super) fn ready_backend(&self) -> Option<Arc<AgentBackend>> {
        if self.startup.closed.load(Ordering::Acquire) {
            return None;
        }
        let receiver = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned")
            .clone()?;
        let state = receiver.borrow().clone();
        match state {
            StartupState::Ready(Ok(backend)) if backend.is_alive() => Some(backend),
            StartupState::Starting | StartupState::Ready(Err(_)) => None,
            StartupState::Ready(Ok(_)) => None,
        }
    }

    #[cfg(test)]
    pub(super) fn retire(&self) {
        let mut receiver = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned");
        self.startup.generation.fetch_add(1, Ordering::AcqRel);
        *receiver = None;
    }

    pub(super) fn thread_start_params(&self, mut params: serde_json::Value) -> serde_json::Value {
        super::route_config::apply(&self.template, &mut params);
        params
    }

    fn is_started(&self) -> bool {
        self.activated.load(Ordering::Relaxed) && self.ready_backend().is_some()
    }

    pub(super) fn is_alive(&self) -> bool {
        if self.startup.closed.load(Ordering::Acquire) {
            return false;
        }
        let Some(startup) = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned")
            .clone()
        else {
            return true;
        };
        match startup.borrow().clone() {
            StartupState::Starting => true,
            StartupState::Ready(Ok(backend)) => backend.is_alive(),
            StartupState::Ready(Err(_)) => false,
        }
    }
}

async fn wait_for_started_backend(
    route: &RoutedBackend,
    mut generation: u64,
    mut startup: tokio::sync::watch::Receiver<StartupState>,
) -> Result<Arc<AgentBackend>> {
    loop {
        let state = startup_state(&mut startup);
        if let Some(backend) = route
            .advance_startup_state(&mut generation, &mut startup, state)
            .await?
        {
            return Ok(backend);
        }
    }
}

fn startup_state(startup: &mut tokio::sync::watch::Receiver<StartupState>) -> StartupState {
    startup.borrow_and_update().clone()
}

#[cfg(test)]
include!("routes_tests.rs");
