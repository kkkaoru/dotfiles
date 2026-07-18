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
    fn lazy(model: String, kind: BackendKind, startup: Arc<BackendStartup>) -> Self {
        Self {
            model,
            kind,
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
                let kind = self.kind;
                let model = self.model.clone();
                tokio::spawn(async move {
                    let result = AgentBackend::spawn(kind, &model)
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
                        route.model.clone(),
                        route.backend,
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
        self.configured.iter().any(|route| route.model == model) || inferred_kind(model).is_some()
    }

    pub(super) fn descriptions(&self) -> Vec<String> {
        self.configured
            .iter()
            .map(|route| format!("{}={}", route.model, route.kind))
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
        let kind = inferred_kind(model)
            .with_context(|| format!("no backend route is configured for model `{model}`"))?;
        let route = Arc::new(RoutedBackend::lazy(
            model.to_owned(),
            kind,
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
        BackendKind::GrokAcp => Arc::new(OnceLock::new()),
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
mod tests {
    use std::sync::Arc;

    use super::{MAX_DYNAMIC_ROUTES, RoutedBackends};
    use crate::agent_backend::{BackendKind, BackendRoute};

    #[test]
    fn shares_codex_provider_startup_but_keeps_grok_model_specific() {
        let routes = RoutedBackends::lazy(&[
            route("gpt-one", BackendKind::CodexAppServer),
            route("gpt-two", BackendKind::CodexAppServer),
            route("grok-one", BackendKind::GrokAcp),
            route("grok-two", BackendKind::GrokAcp),
        ]);
        let gpt_one = routes.route(0);
        let gpt_two = routes.route(1);
        let grok_one = routes.route(2);
        let grok_two = routes.route(3);

        assert!(Arc::ptr_eq(&gpt_one.startup, &gpt_two.startup));
        assert!(!Arc::ptr_eq(&grok_one.startup, &grok_two.startup));

        let (_, dynamic_gpt) = routes.resolve("gpt-dynamic").unwrap();
        let (_, dynamic_grok) = routes.resolve("grok-dynamic").unwrap();
        assert!(Arc::ptr_eq(&gpt_one.startup, &dynamic_gpt.startup));
        assert!(!Arc::ptr_eq(&grok_one.startup, &dynamic_grok.startup));
    }

    #[test]
    fn bounds_dynamic_routes_but_reuses_existing_models() {
        let routes = RoutedBackends::lazy(&[]);
        for index in 0..MAX_DYNAMIC_ROUTES {
            let (route_index, route) = routes
                .resolve(&format!("gpt-dynamic-{index}"))
                .expect("available dynamic route");
            assert_eq!(route_index, index);
            assert_eq!(route.model, format!("gpt-dynamic-{index}"));
        }
        let (existing, _) = routes.resolve("gpt-dynamic-0").expect("existing route");
        assert_eq!(existing, 0);
        assert!(routes.resolve("grok-over-limit").is_err());
    }

    fn route(model: &str, backend: BackendKind) -> BackendRoute {
        BackendRoute {
            model: model.to_owned(),
            backend,
        }
    }
}
