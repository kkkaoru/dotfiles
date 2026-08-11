use std::sync::Arc;

use super::{BackendKind, RoutedBackend, RoutedBackends, WebSearchMode};

impl RoutedBackends {
pub(in crate::agent_backend) fn supports(&self, model: &str) -> bool {
    self.configured.iter().any(|route| {
        route.model == model
            || route
                .template
                .model_prefixes
                .iter()
                .any(|prefix| model.starts_with(prefix))
    })
}

pub(in crate::agent_backend) fn web_search_mode(&self, model: &str) -> WebSearchMode {
    self.find(model)
        .map(|route| route.template.web_search_mode)
        .or_else(|| {
            self.prefix_template(model)
                .map(|route| route.web_search_mode)
        })
        .unwrap_or_default()
}

pub(in crate::agent_backend) fn launch_scoped_effort(&self, model: &str) -> Option<String> {
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

pub(in crate::agent_backend) fn descriptions(&self) -> Vec<String> {
    self.configured
        .iter()
        .map(|route| route.template.description())
        .collect()
}

pub(in crate::agent_backend) fn models(&self) -> Vec<String> {
    let dynamic = self.dynamic.lock().expect("dynamic routes poisoned");
    self.configured
        .iter()
        .chain(dynamic.iter())
        .map(|route| route.model.clone())
        .collect()
}

pub(in crate::agent_backend) fn started_models(&self) -> Vec<String> {
    let dynamic = self.dynamic.lock().expect("dynamic routes poisoned");
    self.configured
        .iter()
        .chain(dynamic.iter())
        .filter(|route| route.is_started())
        .map(|route| route.model.clone())
        .collect()
}

pub(in crate::agent_backend) fn is_alive(&self) -> bool {
    // Routes restart lazily. Marking the whole HTTP daemon unavailable for one failed child
    // would make the launcher terminate unrelated in-flight model streams.
    true
}

pub(in crate::agent_backend) fn model_is_alive(&self, model: &str) -> bool {
    self.find(model).is_none_or(|route| route.is_alive())
}

pub(in crate::agent_backend) fn route(&self, index: usize) -> Arc<RoutedBackend> {
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
