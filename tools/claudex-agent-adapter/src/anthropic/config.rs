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
            .or_else(usage_limit_cooldown::current_cache_path)
    }

    pub(super) fn provider_auth_cache_path(&self) -> Option<PathBuf> {
        self.usage_limit_cache_home
            .as_deref()
            .map(super::provider_auth_cooldown::cache_path_for_home)
            .or_else(super::provider_auth_cooldown::current_cache_path)
    }

    pub(super) fn usage_routing_cache_path(&self) -> Option<PathBuf> {
        #[cfg(test)]
        {
            return self
                .usage_limit_cache_home
                .as_deref()
                .map(super::routing_quota::cache_path_for_home);
        }
        #[cfg(not(test))]
        {
            self.usage_limit_cache_home
                .as_deref()
                .map(super::routing_quota::cache_path_for_home)
                .or_else(super::routing_quota::current_cache_path)
        }
    }
}
