use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};

use super::{AgentBackend, BackendKind, BackendRoute};

const MAX_DYNAMIC_ROUTES: usize = 32;

pub(super) struct RoutedBackend {
    pub(super) model: String,
    pub(super) kind: BackendKind,
    template: BackendRoute,
    startup: Arc<BackendStartup>,
    activated: AtomicBool,
}

type BackendStartup = OnceLock<tokio::sync::watch::Receiver<StartupState>>;

#[derive(Clone)]
enum StartupState {
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
        let startup = Arc::new(OnceLock::new());
        startup.set(receiver).ok().expect("empty startup cell");
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
                StartupState::Ready(Ok(backend)) => return Ok(backend),
                StartupState::Ready(Err(error)) => bail!(error.to_string()),
            }
        }
    }

    fn startup_receiver(&self) -> tokio::sync::watch::Receiver<StartupState> {
        self.startup
            .get_or_init(|| {
                let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
                let route = self.template.clone();
                tokio::spawn(async move {
                    let result = AgentBackend::spawn_route(&route)
                        .await
                        .map_err(|error| Arc::<str>::from(format!("{error:#}")));
                    sender.send_replace(StartupState::Ready(result));
                });
                receiver
            })
            .clone()
    }

    pub(super) fn ready_backend(&self) -> Option<Arc<AgentBackend>> {
        let state = self.startup.get()?.borrow().clone();
        match state {
            StartupState::Ready(Ok(backend)) => Some(backend),
            StartupState::Starting | StartupState::Ready(Err(_)) => None,
        }
    }

    fn is_started(&self) -> bool {
        self.activated.load(Ordering::Relaxed) && self.ready_backend().is_some()
    }

    fn is_alive(&self) -> bool {
        let Some(startup) = self.startup.get() else {
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
        let codex_startup = Arc::new(OnceLock::new());
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
            .unwrap_or_else(|| Arc::new(OnceLock::new()));
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
        }) || inferred_kind(model).is_some()
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
        self.configured.iter().all(|route| route.is_alive())
            && self
                .dynamic
                .lock()
                .expect("dynamic routes poisoned")
                .iter()
                .all(|route| route.is_alive())
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

    pub(super) fn find(&self, model: &str) -> Option<Arc<RoutedBackend>> {
        self.configured
            .iter()
            .find(|route| route.model == model)
            .cloned()
            .or_else(|| {
                self.dynamic
                    .lock()
                    .expect("dynamic routes poisoned")
                    .iter()
                    .find(|route| route.model == model)
                    .cloned()
            })
    }

    pub(super) fn resolve(&self, model: &str) -> Result<(usize, Arc<RoutedBackend>)> {
        if let Some(index) = self
            .configured
            .iter()
            .position(|route| route.model == model)
        {
            return Ok((index, Arc::clone(&self.configured[index])));
        }
        let mut dynamic = self.dynamic.lock().expect("dynamic routes poisoned");
        if let Some(index) = dynamic.iter().position(|route| route.model == model) {
            return Ok((self.configured.len() + index, Arc::clone(&dynamic[index])));
        }
        if dynamic.len() == MAX_DYNAMIC_ROUTES {
            bail!("dynamic backend route limit reached");
        }
        let template = self
            .configured
            .iter()
            .filter(|route| {
                route
                    .template
                    .model_prefixes
                    .iter()
                    .any(|prefix| model.starts_with(prefix))
            })
            .max_by_key(|route| {
                route
                    .template
                    .model_prefixes
                    .iter()
                    .filter(|prefix| model.starts_with(*prefix))
                    .map(String::len)
                    .max()
                    .unwrap_or_default()
            })
            .map(|route| route.template.clone())
            .or_else(|| inferred_kind(model).map(|kind| BackendRoute::new(model, kind)))
            .with_context(|| format!("no backend route is configured for model `{model}`"))?;
        let kind = template.backend;
        let route = Arc::new(RoutedBackend::lazy(
            BackendRoute {
                model: model.to_owned(),
                ..template
            },
            provider_startup(kind, &self.codex_startup),
        ));
        dynamic.push(Arc::clone(&route));
        Ok((self.configured.len() + dynamic.len() - 1, route))
    }

    pub(super) fn first_ready(&self, kind: BackendKind) -> Option<Arc<AgentBackend>> {
        self.configured
            .iter()
            .find(|route| route.kind == kind && route.ready_backend().is_some())
            .and_then(|route| route.ready_backend())
            .or_else(|| {
                self.dynamic
                    .lock()
                    .expect("dynamic routes poisoned")
                    .iter()
                    .find(|route| route.kind == kind && route.ready_backend().is_some())
                    .and_then(|route| route.ready_backend())
            })
    }
}

fn provider_startup(kind: BackendKind, codex_startup: &Arc<BackendStartup>) -> Arc<BackendStartup> {
    match kind {
        BackendKind::CodexAppServer => Arc::clone(codex_startup),
        BackendKind::ConfiguredAcp | BackendKind::CopilotAcp | BackendKind::GrokAcp => {
            Arc::new(OnceLock::new())
        }
    }
}

fn inferred_kind(model: &str) -> Option<BackendKind> {
    if model.starts_with("gpt") {
        Some(BackendKind::CodexAppServer)
    } else if model.starts_with("grok") {
        Some(BackendKind::GrokAcp)
    } else {
        None
    }
}

#[cfg(test)]
include!("routes_tests.rs");
