use super::{
    UsageLimitFailover,
};
use super::support::{ordered_subagent_failover_candidates, push_model_auth_scopes};
use crate::agent_backend::BackendKind;
use crate::anthropic::{
    Bridge, provider_auth, provider_auth_cooldown, request_routing::RouteDecision,
    usage_limit_cooldown,
};
use std::time::SystemTime;

impl Bridge {
    pub(in crate::anthropic) fn usage_limit_failover_for(
        &self,
        exhausted_model: &str,
    ) -> Option<UsageLimitFailover> {
        let _ = exhausted_model;
        // Recover outer turns onto the configured Claude subscription. Sibling
        // ACP providers may also be empty while the subscription remains usable.
        let (model, effort) = self.model_catalog.configured_fallback()?;
        Some(UsageLimitFailover {
            model: model.to_owned(),
            effort: Some(effort.to_owned()),
            route: RouteDecision::Subscription,
        })
    }

    /// Choose failover for an already-open SSE stream.
    /// SubAgents prefer a sibling Provider; outer turns keep subscription fallback.
    pub(in crate::anthropic) fn failover_for_stream_turn(
        &self,
        exhausted_model: &str,
        is_subagent: bool,
    ) -> Option<UsageLimitFailover> {
        if is_subagent {
            self.subagent_provider_failover_for(exhausted_model)
                .or_else(|| self.usage_limit_failover_for(exhausted_model))
        } else {
            self.usage_limit_failover_for(exhausted_model)
        }
    }

    /// Sibling provider for SubAgent empty-ACP / billing failures.
    /// Prefer Qwen Cloud, then other non-exhausted configured ACP routes.
    /// Codex usage-limit recovery keeps using [`Self::usage_limit_failover_for`].
    pub(in crate::anthropic) fn subagent_provider_failover_for(
        &self,
        exhausted_model: &str,
    ) -> Option<UsageLimitFailover> {
        self.subagent_provider_failover_excluding(exhausted_model, None)
    }

    pub(in crate::anthropic) fn subagent_provider_failover_excluding(
        &self,
        exhausted_model: &str,
        quota: Option<&serde_json::Value>,
    ) -> Option<UsageLimitFailover> {
        self.app.backend_kind_for_model(exhausted_model)?;
        let ordered = ordered_subagent_failover_candidates(self);
        ordered.into_iter().find_map(|model| {
            self.subagent_failover_candidate(exhausted_model, quota, model)
        })
    }

    fn subagent_failover_candidate(
        &self,
        exhausted_model: &str,
        quota: Option<&serde_json::Value>,
        model: String,
    ) -> Option<UsageLimitFailover> {
        if model == exhausted_model
            || self.subagent_model_is_exhausted(&model, quota)
            || self.model_concurrency.is_subagent_at_capacity(&model)
        {
            return None;
        }
        let kind = self.app.backend_kind_for_model(&model)?;
        if !super::super::exhausted_subagent::subagent_failover_target_ok(kind) {
            return None;
        }
        let effort = self
            .model_catalog
            .worker_effort_for_model(&model)
            .map(str::to_owned)
            .or_else(|| self.app.launch_scoped_effort(&model));
        Some(UsageLimitFailover {
            model,
            effort,
            route: RouteDecision::Provider,
        })
    }

    pub(in crate::anthropic) fn model_uses_codex_app_server(&self, model: &str) -> bool {
        match self.app.backend_kind_for_model(model) {
            Some(kind) => kind == BackendKind::CodexAppServer,
            None => matches!(&*self.app, crate::agent_backend::AgentBackend::Codex(_)),
        }
    }

    pub(super) fn codex_usage_limit_is_active(&self, model: &str) -> bool {
        self.model_uses_codex_app_server(model)
            && usage_limit_cooldown::codex_app_server_is_cooling_down_at(
                self.usage_limit_cache_path().as_deref(),
                SystemTime::now(),
            )
    }

    pub(super) fn provider_auth_is_cooling_down(&self, model: &str) -> bool {
        let path = self.provider_auth_cache_path();
        let now = SystemTime::now();
        self.auth_scopes_for(Some(model), "").iter().any(|scope| {
            provider_auth_cooldown::scope_is_cooling_down_at(path.as_deref(), scope, now)
        })
    }

    pub(super) fn auth_scopes_for(&self, model: Option<&str>, message: &str) -> Vec<String> {
        let mut scopes = Vec::new();
        push_model_auth_scopes(self, model, &mut scopes);
        if let Some(scope) = provider_auth::auth_scope_from_message(message) {
            scopes.push(scope);
        }
        scopes.sort();
        scopes.dedup();
        scopes
    }
}
