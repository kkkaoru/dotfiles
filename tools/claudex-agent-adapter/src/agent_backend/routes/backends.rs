use std::sync::{Arc, Mutex, atomic::AtomicBool};

use super::{AgentBackend, BackendKind, BackendRoute, BackendStartup, RoutedBackend};

pub struct RoutedBackends {
    pub(super) configured: Vec<Arc<RoutedBackend>>,
    pub(super) dynamic: Mutex<Vec<Arc<RoutedBackend>>>,
    pub(super) search_routes: Mutex<Vec<Arc<RoutedBackend>>>,
    pub(super) codex_startup: Arc<BackendStartup>,
    pub(super) closed: AtomicBool,
    /// Session-scoped configured ACP routes with the same launch contract share
    /// one persistent child. The route remains model-specific for request
    /// routing, while ACP session model selection happens per `thread/start`.
    pub(super) configured_acp_startups: Mutex<Vec<(ConfiguredAcpStartupKey, Arc<BackendStartup>)>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConfiguredAcpStartupKey {
    program: String,
    arguments: Vec<String>,
    max_concurrency: Option<usize>,
}

pub(super) fn configured_acp_startup_key(route: &BackendRoute) -> Option<ConfiguredAcpStartupKey> {
    if route.backend != BackendKind::ConfiguredAcp {
        return None;
    }
    let acp = route.acp.as_ref()?;
    // A model/effort placeholder changes the provider process itself. Such
    // launch-scoped commands retain one child per target route.
    if acp
        .arguments
        .iter()
        .any(|argument| argument.contains("{model}") || argument.contains("{effort}"))
    {
        return None;
    }
    Some(ConfiguredAcpStartupKey {
        program: acp.program.clone(),
        arguments: acp.arguments.clone(),
        max_concurrency: route.max_concurrency,
    })
}

pub(super) fn startup_for_route(
    route: &BackendRoute,
    codex_startup: &Arc<BackendStartup>,
    configured_acp_startups: &Mutex<Vec<(ConfiguredAcpStartupKey, Arc<BackendStartup>)>>,
) -> Arc<BackendStartup> {
    if route.backend == BackendKind::CodexAppServer {
        return Arc::clone(codex_startup);
    }
    let Some(key) = configured_acp_startup_key(route) else {
        return Arc::new(BackendStartup::default());
    };
    let mut startups = configured_acp_startups
        .lock()
        .expect("configured ACP startup poisoned");
    if let Some((_, startup)) = startups.iter().find(|(existing, _)| *existing == key) {
        return Arc::clone(startup);
    }
    let startup = Arc::new(BackendStartup::default());
    startups.push((key, Arc::clone(&startup)));
    startup
}

impl RoutedBackends {
    pub(in crate::agent_backend) fn lazy(routes: &[BackendRoute]) -> Self {
        let codex_startup = Arc::new(BackendStartup::default());
        let configured_acp_startups = Mutex::new(Vec::new());
        Self {
            configured: routes
                .iter()
                .map(|route| {
                    Arc::new(RoutedBackend::lazy(
                        route.clone(),
                        startup_for_route(route, &codex_startup, &configured_acp_startups),
                    ))
                })
                .collect(),
            dynamic: Mutex::new(Vec::new()),
            search_routes: Mutex::new(Vec::new()),
            codex_startup,
            closed: AtomicBool::new(false),
            configured_acp_startups,
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
            search_routes: Mutex::new(Vec::new()),
            codex_startup,
            closed: AtomicBool::new(false),
            configured_acp_startups: Mutex::new(Vec::new()),
        }
    }

    pub(in crate::agent_backend) async fn search_backend(
        &self,
        model: &str,
    ) -> anyhow::Result<Arc<AgentBackend>> {
        self.search_route(model).get().await
    }

    pub(in crate::agent_backend) fn search_route(&self, model: &str) -> Arc<RoutedBackend> {
        let mut search_routes = self.search_routes.lock().expect("search routes poisoned");
        if let Some(existing) = search_routes.iter().find(|route| route.model == model) {
            return Arc::clone(existing);
        }
        let mut template = BackendRoute::new(model, BackendKind::CodexAppServer);
        template.web_search_mode = crate::agent_backend::WebSearchMode::CodexNative;
        let route = Arc::new(RoutedBackend::lazy(
            template,
            Arc::clone(&self.codex_startup),
        ));
        search_routes.push(Arc::clone(&route));
        route
    }
}
