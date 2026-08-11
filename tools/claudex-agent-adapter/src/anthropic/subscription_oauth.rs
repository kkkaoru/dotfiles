use std::{
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Error, Result};
use axum::{body::Body, http::Response};

use super::subscription::failure::subscription_failure;
use super::{
    Bridge, MessagesRequest, provider_auth_cooldown, request_routing::RouteDecision, token_count,
    usage_limit_failover::UsageLimitFailover,
};

#[path = "subscription_oauth_expiry.rs"]
mod expiry;
use expiry::{default_credentials_path, push_unique, warn_preflight_oauth_failover};
pub(super) use expiry::credentials_oauth_unusable_at;
#[cfg(test)]
pub(super) use expiry::credentials_access_expired_at;

pub(super) const SUBSCRIPTION_AUTH_SCOPE: &str = "claude-subscription";

/// Exact Claude Code / TUI wording from horse-racing `fa522331-…`.
const TUI_OAUTH_EXPIRED: &str = "oauth session expired and could not be refreshed";
const TUI_LOGIN_PROMPT: &str = "please run /login";

pub(super) fn is_subscription_auth_failure(error: &Error) -> bool {
    if let Some(failure) = subscription_failure(error) {
        return failure.is_authentication();
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains(TUI_OAUTH_EXPIRED)
        || message.contains(TUI_LOGIN_PROMPT)
        || (message.contains("oauth") && message.contains("expired"))
}


impl Bridge {
    fn subscription_credentials_path(&self) -> Option<PathBuf> {
        self.usage_limit_cache_home
            .as_ref()
            .map(|home| home.join(".claude/.credentials.json"))
            .or_else(default_credentials_path)
    }

    pub(super) fn subscription_oauth_is_unusable(&self) -> bool {
        match self
            .subscription_credentials_path()
            .and_then(|path| credentials_oauth_unusable_at(&path, SystemTime::now()))
        {
            Some(false) => false,
            Some(true) => true,
            None => provider_auth_cooldown::scope_is_cooling_down_at(
                self.provider_auth_cache_path().as_deref(),
                SUBSCRIPTION_AUTH_SCOPE,
                SystemTime::now(),
            ),
        }
    }

    pub(super) fn apply_subscription_auth_preflight(
        &self,
        request: &mut MessagesRequest,
        route: RouteDecision,
        effort: &mut Option<String>,
    ) -> RouteDecision {
        if route != RouteDecision::Subscription || !self.subscription_oauth_is_unusable() {
            return route;
        }
        let Some(failover) = self.subscription_auth_failover_for() else {
            tracing::warn!(
                model = %request.model,
                "Claude subscription OAuth is unusable but no provider failover is configured"
            );
            return route;
        };
        warn_preflight_oauth_failover(&request.model, &failover.model);
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        failover.route
    }

    pub(super) fn subscription_auth_failover_for(&self) -> Option<UsageLimitFailover> {
        let mut candidates = Vec::new();
        for worker in self.model_catalog.worker_routes() {
            candidates.push(worker.model.clone());
        }
        for model in self.app.models() {
            push_unique(&mut candidates, model);
        }
        candidates
            .into_iter()
            .find_map(|model| self.provider_failover_candidate(model))
    }

    fn provider_failover_candidate(&self, model: String) -> Option<UsageLimitFailover> {
        if self.subagent_provider_is_exhausted(&model) {
            return None;
        }
        self.app.backend_kind_for_model(&model)?;
        let effort = self
            .model_catalog
            .worker_effort_for_model(&model)
            .map(str::to_owned)
            .or_else(|| self.app.launch_scoped_effort(&model));
        Some(UsageLimitFailover {
            effort,
            model,
            route: RouteDecision::Provider,
        })
    }

    pub(super) async fn subscription_messages_with_auth_failover(
        self: &Arc<Self>,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        let can_failover = !request.stream;
        let failover_request = can_failover.then(|| request.clone());
        match self
            .subscription_messages(request, effort.clone(), is_subagent, tools_were_provided)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if can_failover && is_subscription_auth_failure(&error) => {
                self.retry_after_subscription_auth_failure(
                    error,
                    failover_request,
                    effort,
                    is_subagent,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn retry_after_subscription_auth_failure(
        self: &Arc<Self>,
        error: anyhow::Error,
        failover_request: Option<MessagesRequest>,
        effort: Option<String>,
        is_subagent: bool,
    ) -> Result<Response<Body>> {
        self.note_provider_exhaustion(&error, None);
        let Some(mut retry) = failover_request else {
            return Err(error);
        };
        let Some(failover) = self.subscription_auth_failover_for() else {
            return Err(error);
        };
        tracing::warn!(
            failover_model = %failover.model,
            "failing over outer non-stream turn after Claude subscription OAuth failure"
        );
        retry.model = failover.model;
        let failover_effort = failover.effort.or(effort);
        let input_tokens = u64::try_from(token_count(&retry)).unwrap_or(u64::MAX);
        self.provider_messages(retry, input_tokens, failover_effort, is_subagent, false)
            .await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "subscription_oauth_tests.rs"]
mod tests;
