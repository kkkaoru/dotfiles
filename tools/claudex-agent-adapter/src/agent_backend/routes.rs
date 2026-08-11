use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};

use super::{AgentBackend, BackendKind, BackendRoute, WebSearchMode};

mod concurrency;
mod resolve;
mod shutdown;
mod startup;

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
    fn lazy(route: BackendRoute, startup: Arc<BackendStartup>) -> Self {
        Self {
            model: route.model.clone(),
            kind: route.backend,
            template: route,
            startup,
            activated: AtomicBool::new(false),
        }
    }

    fn ready(model: String, backend: Arc<AgentBackend>) -> Self {
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

pub struct RoutedBackends {
    configured: Vec<Arc<RoutedBackend>>,
    dynamic: Mutex<Vec<Arc<RoutedBackend>>>,
    codex_startup: Arc<BackendStartup>,
}

impl RoutedBackends {
    pub(super) fn lazy(routes: &[BackendRoute]) -> Self {
        let codex_startup = Arc::new(BackendStartup::default());
        Self {
            configured: routes
                .iter()
                .map(|route| {
                    Arc::new(RoutedBackend::lazy(
                        route.clone(),
                        provider_startup(route.backend, &codex_startup),
                    ))
                })
                .collect(),
            dynamic: Mutex::new(Vec::new()),
            codex_startup,
        }
    }

    pub(super) fn ready(routes: Vec<(String, Arc<AgentBackend>)>) -> Self {
        let configured = routes
            .into_iter()
            .map(|(model, backend)| Arc::new(RoutedBackend::ready(model, backend)))
            .collect::<Vec<_>>();
        let codex_startup = configured
            .iter()
            .find(|route| route.kind == BackendKind::CodexAppServer)
            .map(|route| Arc::clone(&route.startup))
            .unwrap_or_else(|| Arc::new(BackendStartup::default()));
        Self {
            configured,
            dynamic: Mutex::new(Vec::new()),
            codex_startup,
        }
    }

    pub(super) fn supports(&self, model: &str) -> bool {
        self.configured.iter().any(|route| {
            route.model == model
                || route
                    .template
                    .model_prefixes
                    .iter()
                    .any(|prefix| model.starts_with(prefix))
        })
    }

    pub(super) fn web_search_mode(&self, model: &str) -> WebSearchMode {
        self.find(model)
            .map(|route| route.template.web_search_mode)
            .or_else(|| {
                self.prefix_template(model)
                    .map(|route| route.web_search_mode)
            })
            .unwrap_or_default()
    }

    pub(super) fn launch_scoped_effort(&self, model: &str) -> Option<String> {
        let route = self
            .find(model)
            .map(|r| r.template.clone())
            .or_else(|| self.prefix_template(model).cloned())?;
        let pins = route.backend == BackendKind::GrokAcp
            || route
                .acp
                .as_ref()
                .is_some_and(|a| a.arguments.iter().any(|x| x.contains("{effort}")));
        pins.then_some(route.effort).flatten()
    }

    pub(super) fn descriptions(&self) -> Vec<String> {
        self.configured
            .iter()
            .map(|route| route.template.description())
            .collect()
    }

    pub(super) fn models(&self) -> Vec<String> {
        let dynamic = self.dynamic.lock().expect("dynamic routes poisoned");
        self.configured
            .iter()
            .chain(dynamic.iter())
            .map(|route| route.model.clone())
            .collect()
    }

    pub(super) fn started_models(&self) -> Vec<String> {
        let dynamic = self.dynamic.lock().expect("dynamic routes poisoned");
        self.configured
            .iter()
            .chain(dynamic.iter())
            .filter(|route| route.is_started())
            .map(|route| route.model.clone())
            .collect()
    }

    pub(super) fn is_alive(&self) -> bool {
        // Routes restart lazily. Marking the whole HTTP daemon unavailable for one failed child
        // would make the launcher terminate unrelated in-flight model streams.
        true
    }

    pub(super) fn model_is_alive(&self, model: &str) -> bool {
        self.find(model).is_none_or(|route| route.is_alive())
    }

    pub(super) fn route(&self, index: usize) -> Arc<RoutedBackend> {
        if let Some(route) = self.configured.get(index) {
            return Arc::clone(route);
        }
        self.dynamic
            .lock()
            .expect("dynamic routes poisoned")
            .get(index - self.configured.len())
            .cloned()
            .expect("routed backend index must exist")
    }

}

#[cfg(test)]
include!("routes_tests.rs");
