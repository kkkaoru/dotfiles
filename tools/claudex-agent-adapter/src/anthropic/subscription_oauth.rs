use std::{path::PathBuf, time::SystemTime};

use anyhow::Error;

use super::subscription::failure::subscription_failure;
use super::{Bridge, MessagesRequest, provider_auth_cooldown, request_routing::RouteDecision};

#[path = "subscription_oauth_expiry.rs"]
mod expiry;
#[cfg(test)]
pub(super) use expiry::credentials_access_expired_at;
pub(super) use expiry::credentials_oauth_unusable_at;
#[allow(unused_imports)] // failover submodule reads super::push_unique
use expiry::{default_credentials_path, push_unique, warn_preflight_oauth_failover};

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

#[path = "subscription_oauth_failover.rs"]
mod failover;

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
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "subscription_oauth_tests.rs"]
mod tests;
