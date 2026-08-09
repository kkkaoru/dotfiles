use std::{sync::Arc, time::SystemTime};

use anyhow::Result;
use axum::{body::Body, http::Response};

use crate::agent_backend::BackendKind;

use super::{
    Bridge, MessagesRequest, provider_auth, provider_auth_cooldown,
    request_routing::RouteDecision,
    segment::contains_empty_acp_billing_marker,
    stream::usage_limit::{
        contains_classic_usage_limit_marker, contains_provider_quota_exhausted_marker,
        contains_rate_limit_marker, contains_usage_limit_marker,
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
        if super::subscription_oauth::is_subscription_auth_failure(error) {
            if let Some(path) = provider_auth_cooldown::record_at(
                self.provider_auth_cache_path().as_deref(),
                super::subscription_oauth::SUBSCRIPTION_AUTH_SCOPE,
                &message,
                SystemTime::now(),
            ) {
                tracing::warn!(
                    path = %path.display(),
                    auth_scope = super::subscription_oauth::SUBSCRIPTION_AUTH_SCOPE,
                    reason = "subscription-oauth",
                    error = %message,
                    "recorded Claude subscription OAuth cooldown; routing outer turns onto providers"
                );
            }
        }
        if contains_rate_limit_marker(&message)
            || provider_auth::contains_auth_failure_marker(&message)
            || contains_empty_acp_billing_marker(&message)
            || contains_provider_quota_exhausted_marker(&message)
        {
            let scopes = self.auth_scopes_for(exhausted_model, &message);
            let cache_path = self.provider_auth_cache_path();
            let rate_limited = contains_rate_limit_marker(&message);
            let quota_exhausted = contains_provider_quota_exhausted_marker(&message);
            let reason = if rate_limited {
                "rate-limit"
            } else if quota_exhausted {
                "quota"
            } else if contains_empty_acp_billing_marker(&message) {
                "empty-acp-billing"
            } else {
                "auth"
            };
            for scope in scopes {
                let recorded = if rate_limited || quota_exhausted {
                    provider_auth_cooldown::record_rate_limit_at(
                        cache_path.as_deref(),
                        &scope,
                        &message,
                        SystemTime::now(),
                    )
                } else {
                    provider_auth_cooldown::record_at(
                        cache_path.as_deref(),
                        &scope,
                        &message,
                        SystemTime::now(),
                    )
                };
                if let Some(path) = recorded {
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
        self.subagent_model_is_exhausted(model, None)
    }

    pub(super) fn subagent_model_is_exhausted(
        &self,
        model: &str,
        quota: Option<&serde_json::Value>,
    ) -> bool {
        self.provider_auth_is_cooling_down(model)
            || self.codex_usage_limit_is_active(model)
            || super::routing_quota::live_cache_marks_model_exhausted(
                self.usage_routing_cache_path().as_deref(),
                model,
                SystemTime::now(),
            )
            || quota.is_some_and(|summary| {
                super::routing_quota::summary_marks_model_exhausted(summary, model)
            })
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

    /// Choose failover for an already-open SSE stream.
    /// SubAgents prefer a sibling Provider; outer turns keep subscription fallback.
    pub(super) fn failover_for_stream_turn(
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
    pub(super) fn subagent_provider_failover_for(
        &self,
        exhausted_model: &str,
    ) -> Option<UsageLimitFailover> {
        self.subagent_provider_failover_excluding(exhausted_model, None)
    }

    pub(super) fn subagent_provider_failover_excluding(
        &self,
        exhausted_model: &str,
        quota: Option<&serde_json::Value>,
    ) -> Option<UsageLimitFailover> {
        let exhausted_kind = self.app.backend_kind_for_model(exhausted_model)?;
        if !super::exhausted_subagent::subagent_failover_source_ok(exhausted_kind) {
            return None;
        }
        const PREFERRED: &[&str] = &["qwen3.8-max-preview"];
        let mut candidates = Vec::new();
        for model in PREFERRED {
            candidates.push((*model).to_owned());
        }
        for worker in self.model_catalog.worker_routes() {
            candidates.push(worker.model.clone());
        }
        for model in self.app.models() {
            candidates.push(model);
        }
        candidates.sort();
        candidates.dedup();
        let mut ordered = Vec::new();
        for model in PREFERRED {
            if candidates.iter().any(|candidate| candidate == model) {
                ordered.push((*model).to_owned());
            }
        }
        for model in candidates {
            if !ordered.iter().any(|preferred| preferred == &model) {
                ordered.push(model);
            }
        }
        ordered.into_iter().find_map(|model| {
            if model == exhausted_model
                || self.subagent_model_is_exhausted(&model, quota)
                || self.model_concurrency.is_subagent_at_capacity(&model)
            {
                return None;
            }
            let kind = self.app.backend_kind_for_model(&model)?;
            if !super::exhausted_subagent::subagent_failover_target_ok(kind) {
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
    contains_usage_limit_marker(&message)
        || provider_auth::contains_auth_failure_marker(&message)
        || contains_empty_acp_billing_marker(&message)
}

/// Streaming turns can only continue on a sibling Provider.
/// Subscription failover cannot attach to an already-open SubAgent SSE stream,
/// which is why the old empty-ACP path returned the error to Claude Code.
pub(super) fn streaming_provider_retry(
    failover: Option<UsageLimitFailover>,
) -> Option<UsageLimitFailover> {
    failover.filter(|candidate| candidate.route == RouteDecision::Provider)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "usage_limit_failover_tests.rs"]
mod tests;
