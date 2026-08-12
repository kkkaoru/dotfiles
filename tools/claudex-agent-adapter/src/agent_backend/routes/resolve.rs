use std::sync::Arc;

use anyhow::{Context, Result, bail};

use super::{
    BackendRoute, MAX_DYNAMIC_ROUTES, RoutedBackend, RoutedBackends, backends::startup_for_route,
};

impl RoutedBackends {
    pub(in crate::agent_backend) fn max_context_tokens_for_model(
        &self,
        model: &str,
    ) -> Option<u64> {
        let exact_limit = self
            .configured
            .iter()
            .find(|route| route.model == model)
            .and_then(|route| route.template.max_context_tokens)
            .map(|limit| (usize::MAX, limit));
        let prefix_limit = self
            .configured
            .iter()
            .filter_map(|route| {
                route
                    .template
                    .model_prefixes
                    .iter()
                    .filter(|prefix| model.starts_with(prefix.as_str()))
                    .map(String::len)
                    .max()
                    .map(|len| (len, route.template.max_context_tokens))
            })
            .filter(|(_, limit)| limit.is_some())
            .max_by_key(|(len, _)| *len)
            .and_then(|(_, limit)| limit);
        if let Some((_, limit)) = exact_limit {
            return Some(limit);
        }
        self.dynamic
            .lock()
            .expect("dynamic routes poisoned")
            .iter()
            .find(|route| route.model == model)
            .and_then(|route| route.template.max_context_tokens)
            .or(prefix_limit)
    }

    pub(in crate::agent_backend) fn find(&self, model: &str) -> Option<Arc<RoutedBackend>> {
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

    pub(in crate::agent_backend) fn resolve(
        &self,
        model: &str,
    ) -> Result<(usize, Arc<RoutedBackend>)> {
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
            .prefix_template(model)
            .cloned()
            .with_context(|| format!("no backend route is configured for model `{model}`"))?;
        let route = BackendRoute {
            model: model.to_owned(),
            ..template
        };
        // Keep dynamic models on the same configured ACP child when the launch
        // contract is session-scoped and identical.
        let startup = startup_for_route(&route, &self.codex_startup, &self.configured_acp_startups);
        let route = Arc::new(RoutedBackend::lazy(route, startup));
        dynamic.push(Arc::clone(&route));
        Ok((self.configured.len() + dynamic.len() - 1, route))
    }

    pub(super) fn prefix_template(&self, model: &str) -> Option<&BackendRoute> {
        self.configured
            .iter()
            .filter_map(|route| {
                route
                    .template
                    .model_prefixes
                    .iter()
                    .filter(|prefix| model.starts_with(prefix.as_str()))
                    .map(String::len)
                    .max()
                    .map(|length| (length, &route.template))
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, template)| template)
    }
}
