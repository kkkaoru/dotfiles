use std::sync::{Arc, Mutex, atomic::AtomicBool};

use super::{
    AgentBackend, BackendKind, BackendRoute, BackendStartup, RoutedBackend, provider_startup,
};

pub struct RoutedBackends {
    pub(super) configured: Vec<Arc<RoutedBackend>>,
    pub(super) dynamic: Mutex<Vec<Arc<RoutedBackend>>>,
    pub(super) codex_startup: Arc<BackendStartup>,
    pub(super) closed: AtomicBool,
}

impl RoutedBackends {
    pub(in crate::agent_backend) fn lazy(routes: &[BackendRoute]) -> Self {
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
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::agent_backend) fn ready(routes: Vec<(String, Arc<AgentBackend>)>) -> Self {
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
            closed: AtomicBool::new(false),
        }
    }
}
