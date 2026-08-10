use std::{path::PathBuf, sync::Arc};

use super::{Bridge, MessagesRequest, usage_limit_cooldown};

impl Bridge {
    /// Overrides the Claude Code settings source, primarily for isolated runtimes and tests.
    #[must_use]
    pub fn with_settings_path(self, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: Some(settings_path.into()),
            ..self
        }
    }

    /// Override the usage-limit cooldown home for isolated runtimes and tests.
    #[must_use]
    pub fn with_usage_limit_cache_home(self, home: impl Into<PathBuf>) -> Self {
        Self {
            usage_limit_cache_home: Some(home.into()),
            ..self
        }
    }

    /// Install the normalized hard timeout used only for native background
    /// SubAgent requests. `None` deliberately leaves the Claude Code Agent
    /// lifecycle unbounded by claudex.
    #[must_use]
    pub(crate) fn with_subagent_hard_timeout(
        self,
        subagent_hard_timeout: Option<std::time::Duration>,
    ) -> Self {
        Self {
            subagent_hard_timeout,
            ..self
        }
    }

    pub(crate) fn subagent_hard_timeout_seconds(&self) -> Option<u64> {
        self.subagent_hard_timeout.map(|timeout| timeout.as_secs())
    }

    /// Install the config-declared model catalog used for unrouted-provider remaps.
    #[must_use]
    pub fn with_model_catalog(self, model_catalog: crate::provider_config::ModelCatalog) -> Self {
        Self {
            model_catalog,
            ..self
        }
    }

    pub(super) fn request_model(&self, request: &MessagesRequest) -> String {
        if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        }
    }

    pub(super) fn intern_signature(&self, signature: String) -> Arc<str> {
        super::intern_signature(&self.signature_pool, signature)
    }

    pub(super) fn usage_limit_cache_path(&self) -> Option<PathBuf> {
        self.usage_limit_cache_home
            .as_deref()
            .map(usage_limit_cooldown::cache_path_for_home)
            .or_else(default_usage_limit_cache_path)
    }

    pub(super) fn provider_auth_cache_path(&self) -> Option<PathBuf> {
        self.usage_limit_cache_home
            .as_deref()
            .map(super::provider_auth_cooldown::cache_path_for_home)
            .or_else(default_provider_auth_cache_path)
    }

    pub(super) fn usage_routing_cache_path(&self) -> Option<PathBuf> {
        self.usage_limit_cache_home
            .as_deref()
            .map(super::routing_quota::cache_path_for_home)
            .or_else(default_usage_routing_cache_path)
    }
}

#[cfg(test)]
fn default_usage_limit_cache_path() -> Option<PathBuf> {
    None
}

#[cfg(not(test))]
fn default_usage_limit_cache_path() -> Option<PathBuf> {
    usage_limit_cooldown::current_cache_path()
}

#[cfg(test)]
fn default_provider_auth_cache_path() -> Option<PathBuf> {
    None
}

#[cfg(not(test))]
fn default_provider_auth_cache_path() -> Option<PathBuf> {
    super::provider_auth_cooldown::current_cache_path()
}

#[cfg(test)]
fn default_usage_routing_cache_path() -> Option<PathBuf> {
    None
}

#[cfg(not(test))]
fn default_usage_routing_cache_path() -> Option<PathBuf> {
    super::routing_quota::current_cache_path()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::Value;

    use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
    use crate::anthropic::{Bridge, MessagesRequest};

    #[test]
    fn request_model_falls_back_to_bridge_default_when_empty() {
        let bridge = Bridge::new_with_backend(
            AgentBackend::spawn_routes(&[BackendRoute::new(
                "gpt-5.6-luna",
                BackendKind::CodexAppServer,
            )]),
            "gpt-5.6-luna".to_owned(),
        );
        let request = MessagesRequest {
            model: String::new(),
            system: Value::Null,
            messages: Vec::new(),
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };
        assert_eq!(bridge.request_model(&request), "gpt-5.6-luna");
        let named = MessagesRequest {
            model: "gpt-5.6-terra".to_owned(),
            ..request
        };
        assert_eq!(bridge.request_model(&named), "gpt-5.6-terra");
    }
}
