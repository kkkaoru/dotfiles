use std::{sync::Arc, time::SystemTime};

use anyhow::Result;
use axum::{body::Body, http::Response};

use crate::agent_backend::BackendKind;

use super::{
    Bridge, MessagesRequest, provider_auth, provider_auth_cooldown, request_routing::RouteDecision,
    stream::usage_limit::{
        contains_classic_usage_limit_marker, contains_rate_limit_marker, contains_usage_limit_marker,
    },
    token_count, usage_limit_cooldown,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct UsageLimitFailover {
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) route: RouteDecision,
}

impl Bridge {
    pub(super) fn note_provider_exhaustion(
        &self,
        error: &anyhow::Error,
        exhausted_model: Option<&str>,
    ) {
        let message = error.to_string();
        // Classic ChatGPT/Codex usage limits apply to the whole app-server backend.
        // Provider 429s are model/provider scoped so a glm Ollama limit does not
        // cool down unrelated Codex GPT routes that share the same backend.
        if contains_classic_usage_limit_marker(&message) {
            let cache_path = self.usage_limit_cache_path();
            if let Some(path) = usage_limit_cooldown::record_codex_app_server_limit_at(
                cache_path.as_deref(),
                &message,
                SystemTime::now(),
            ) {
                tracing::warn!(
                    path = %path.display(),
                    error = %message,
                    "recorded Codex app-server usage-limit cooldown; routing away from that backend"
                );
            }
        }
        if contains_rate_limit_marker(&message)
            || provider_auth::contains_auth_failure_marker(&message)
        {
            let scopes = self.auth_scopes_for(exhausted_model, &message);
            let cache_path = self.provider_auth_cache_path();
            let reason = if contains_rate_limit_marker(&message) {
                "rate-limit"
            } else {
                "auth"
            };
            for scope in scopes {
                if let Some(path) = provider_auth_cooldown::record_at(
                    cache_path.as_deref(),
                    &scope,
                    &message,
                    SystemTime::now(),
                ) {
                    tracing::warn!(
                        path = %path.display(),
                        auth_scope = %scope,
                        reason,
                        error = %message,
                        "recorded provider exhaustion cooldown; routing away from that provider"
                    );
                }
            }
        }
    }

    pub(super) fn subagent_provider_is_exhausted(&self, model: &str) -> bool {
        self.provider_auth_is_cooling_down(model) || self.codex_usage_limit_is_active(model)
    }

    pub(super) fn apply_usage_limit_preflight(
        &self,
        request: &mut MessagesRequest,
        route: RouteDecision,
        effort: &mut Option<String>,
        is_subagent: bool,
    ) -> RouteDecision {
        // SubAgents keep their routed provider; the orchestrator re-routes from
        // capacity context instead of burning Claude subscription quota.
        if is_subagent || route != RouteDecision::Provider {
            return route;
        }
        let reason = if self.codex_usage_limit_is_active(&request.model) {
            Some("usage-limit")
        } else if self.provider_auth_is_cooling_down(&request.model) {
            Some("auth")
        } else {
            None
        };
        let Some(reason) = reason else {
            return route;
        };
        let Some(failover) = self.usage_limit_failover_for(&request.model) else {
            tracing::warn!(
                model = %request.model,
                reason,
                "provider is cooling down but no failover target is configured"
            );
            return route;
        };
        tracing::warn!(
            exhausted_model = %request.model,
            failover_model = %failover.model,
            failover_route = ?failover.route,
            reason,
            "preflight failover away from exhausted provider"
        );
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        failover.route
    }

    pub(super) async fn provider_messages_with_usage_limit_failover(
        self: &Arc<Self>,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
        run_in_background: bool,
    ) -> Result<Response<Body>> {
        let can_failover = !is_subagent && !request.stream;
        let failover_request = can_failover.then(|| request.clone());
        let exhausted_model = request.model.clone();
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        match self
            .provider_messages(
                request,
                input_tokens,
                effort.clone(),
                is_subagent,
                run_in_background,
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if can_failover && should_failover_provider_error(&error) => {
                self.note_provider_exhaustion(&error, Some(&exhausted_model));
                let Some(mut retry) = failover_request else {
                    return Err(error);
                };
                let Some(failover) = self.usage_limit_failover_for(&exhausted_model) else {
                    return Err(error);
                };
                tracing::warn!(
                    target: "claudex.provider",
                    log_event = "provider_failover",
                    exhausted_model = %exhausted_model,
                    failover_model = %failover.model,
                    failover_route = ?failover.route,
                    "failing over outer non-stream turn after provider exhaustion"
                );
                retry.model = failover.model;
                let failover_effort = failover.effort.or(effort);
                if failover.route == RouteDecision::Subscription {
                    self.subscription_messages(retry, failover_effort, false, tools_were_provided)
                        .await
                } else {
                    let input_tokens = u64::try_from(token_count(&retry)).unwrap_or(u64::MAX);
                    self.provider_messages(retry, input_tokens, failover_effort, false, false)
                        .await
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn usage_limit_failover_for(
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

    pub(super) fn model_uses_codex_app_server(&self, model: &str) -> bool {
        match self.app.backend_kind_for_model(model) {
            Some(kind) => kind == BackendKind::CodexAppServer,
            None => matches!(&*self.app, crate::agent_backend::AgentBackend::Codex(_)),
        }
    }

    fn codex_usage_limit_is_active(&self, model: &str) -> bool {
        self.model_uses_codex_app_server(model)
            && usage_limit_cooldown::codex_app_server_is_cooling_down_at(
                self.usage_limit_cache_path().as_deref(),
                SystemTime::now(),
            )
    }

    fn provider_auth_is_cooling_down(&self, model: &str) -> bool {
        let path = self.provider_auth_cache_path();
        let now = SystemTime::now();
        self.auth_scopes_for(Some(model), "").iter().any(|scope| {
            provider_auth_cooldown::scope_is_cooling_down_at(path.as_deref(), scope, now)
        })
    }

    fn auth_scopes_for(&self, model: Option<&str>, message: &str) -> Vec<String> {
        let mut scopes = Vec::new();
        if let Some(model) = model {
            scopes.push(model.to_owned());
            if let Some(provider) = self.app.model_provider_for_model(model) {
                scopes.push(provider);
            }
        }
        if let Some(scope) = provider_auth::auth_scope_from_message(message) {
            scopes.push(scope);
        }
        scopes.sort();
        scopes.dedup();
        scopes
    }
}

pub(crate) fn is_usage_limit_exceeded(error: &anyhow::Error) -> bool {
    should_failover_provider_error(error)
}

pub(crate) fn should_failover_provider_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    contains_usage_limit_marker(&message) || provider_auth::contains_auth_failure_marker(&message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
    use crate::anthropic::Bridge;
    use crate::anthropic::request_routing::RouteDecision;
    use crate::provider_config::ModelCatalog;

    #[test]
    fn prefers_configured_subscription_fallback_before_other_providers() {
        let backend = AgentBackend::spawn_routes(&[
            BackendRoute::new("fugu", BackendKind::CodexAppServer),
            BackendRoute::new("grok-4.5", BackendKind::GrokAcp),
        ]);
        let mut catalog = ModelCatalog::default();
        catalog
            .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-sonnet",
                "claude-sonnet-5",
                "high",
            )])
            .expect("install fallback");
        let bridge =
            Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
        let failover = bridge
            .usage_limit_failover_for("fugu")
            .expect("failover target");
        assert_eq!(failover.model, "claude-sonnet-5");
        assert_eq!(failover.route, RouteDecision::Subscription);
    }

    #[test]
    fn falls_back_to_configured_subscription_when_only_codex_remains() {
        let backend =
            AgentBackend::spawn_routes(&[BackendRoute::new("fugu", BackendKind::CodexAppServer)]);
        let mut catalog = ModelCatalog::default();
        catalog
            .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-sonnet",
                "claude-sonnet-5",
                "high",
            )])
            .expect("install fallback");
        let bridge =
            Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
        let failover = bridge
            .usage_limit_failover_for("fugu")
            .expect("subscription failover");
        assert_eq!(failover.model, "claude-sonnet-5");
        assert_eq!(failover.route, RouteDecision::Subscription);
    }

    #[test]
    fn treats_sakana_invalid_api_key_as_failover_trigger() {
        assert!(super::should_failover_provider_error(&anyhow::anyhow!(
            "codex app-server turn failed: unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
        )));
    }

    #[test]
    fn treats_429_rate_limit_as_failover_trigger() {
        assert!(super::should_failover_provider_error(&anyhow::anyhow!(
            "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests, request id: abc"
        )));
        assert!(super::should_failover_provider_error(&anyhow::Error::msg(
            r#"codex app-server turn failed: {"error":{"codexErrorInfo":{"responseTooManyFailedAttempts":{"httpStatusCode":429}},"message":"exceeded retry limit"}}"#
        )));
    }

    #[test]
    fn records_429_cooldown_per_model_without_backend_usage_limit() {
        use std::time::SystemTime;

        use crate::anthropic::{provider_auth_cooldown, usage_limit_cooldown};

        let root = tempfile::tempdir().expect("rate-limit cooldown fixture");
        let mut route = BackendRoute::new("glm-5.2:cloud", BackendKind::CodexAppServer);
        route.model_provider = Some("ollama".to_owned());
        let backend = AgentBackend::spawn_routes(&[route]);
        let bridge = Bridge::new_with_backend(backend, "glm-5.2:cloud".to_owned())
            .with_usage_limit_cache_home(root.path());
        bridge.note_provider_exhaustion(
            &anyhow::anyhow!(
                "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests"
            ),
            Some("glm-5.2:cloud"),
        );
        assert!(bridge.subagent_provider_is_exhausted("glm-5.2:cloud"));
        assert!(provider_auth_cooldown::scope_is_cooling_down_at(
            bridge.provider_auth_cache_path().as_deref(),
            "glm-5.2:cloud",
            SystemTime::now(),
        ));
        assert!(provider_auth_cooldown::scope_is_cooling_down_at(
            bridge.provider_auth_cache_path().as_deref(),
            "ollama",
            SystemTime::now(),
        ));
        assert!(!usage_limit_cooldown::codex_app_server_is_cooling_down_at(
            bridge.usage_limit_cache_path().as_deref(),
            SystemTime::now(),
        ));
    }
}
