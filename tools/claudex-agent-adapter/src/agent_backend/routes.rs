use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};

use super::{AgentBackend, BackendKind, BackendRoute, WebSearchMode};

mod backends;
mod concurrency;
mod query;
mod resolve;
mod shutdown;
mod startup;

pub use backends::RoutedBackends;
use startup::{provider_startup, start_backend};

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
        if let Some(backend) = self.ready_backend() {
            return Ok(backend);
        }
        let mut startup = self.startup_receiver();
        loop {
            let state = startup.borrow_and_update().clone();
            match state {
                StartupState::Starting => startup
                    .changed()
                    .await
                    .context("backend startup task stopped without a result")?,
                StartupState::Ready(Ok(backend)) if backend.is_alive() => return Ok(backend),
                StartupState::Ready(Ok(_)) => startup = self.startup_receiver(),
                StartupState::Ready(Err(error)) => bail!(error.to_string()),
            }
        }
    }

    fn startup_receiver(&self) -> tokio::sync::watch::Receiver<StartupState> {
        let mut startup = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned");
        let reusable = startup
            .as_ref()
            .is_some_and(|receiver| match receiver.borrow().clone() {
                StartupState::Starting => true,
                StartupState::Ready(Ok(backend)) => backend.is_alive(),
                StartupState::Ready(Err(_)) => false,
            });
        if !reusable {
            *startup = Some(start_backend(self.template.clone()));
        }
        startup.as_ref().expect("backend startup receiver").clone()
    }

    pub(super) fn ready_backend(&self) -> Option<Arc<AgentBackend>> {
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

    pub(super) fn retire(&self) {
        *self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned") = None;
    }

    pub(super) fn thread_start_params(&self, mut params: serde_json::Value) -> serde_json::Value {
        super::route_config::apply(&self.template, &mut params);
        params
    }

    fn is_started(&self) -> bool {
        self.activated.load(Ordering::Relaxed) && self.ready_backend().is_some()
    }

    pub(super) fn is_alive(&self) -> bool {
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

#[cfg(test)]
include!("routes_tests.rs");
