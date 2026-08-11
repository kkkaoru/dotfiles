use std::time::SystemTime;

use super::support::{
    is_scoped_provider_exhaustion, record_scoped_exhaustion, scoped_exhaustion_reason,
};
use crate::anthropic::{
    Bridge, provider_auth_cooldown,
    stream::usage_limit::{
        contains_classic_usage_limit_marker, contains_provider_quota_exhausted_marker,
        contains_rate_limit_marker,
    },
    usage_limit_cooldown,
};

impl Bridge {
    pub(in crate::anthropic) fn note_provider_exhaustion(
        &self,
        error: &anyhow::Error,
        exhausted_model: Option<&str>,
    ) {
        let message = error.to_string();
        // Classic ChatGPT/Codex usage limits apply to the whole app-server backend.
        // Provider 429s are model/provider scoped so a glm Ollama limit does not
        // cool down unrelated Codex GPT routes that share the same backend.
        self.note_classic_usage_limit(&message);
        self.note_subscription_auth_cooldown(error, &message);
        self.note_scoped_provider_exhaustion(exhausted_model, &message);
    }

    pub(super) fn note_classic_usage_limit(&self, message: &str) {
        if !contains_classic_usage_limit_marker(message) {
            return;
        }
        let cache_path = self.usage_limit_cache_path();
        let Some(path) = usage_limit_cooldown::record_codex_app_server_limit_at(
            cache_path.as_deref(),
            message,
            SystemTime::now(),
        ) else {
            return;
        };
        tracing::warn!(
            path = %path.display(),
            error = %message,
            "recorded Codex app-server usage-limit cooldown; routing away from that backend"
        );
    }

    pub(super) fn note_subscription_auth_cooldown(&self, error: &anyhow::Error, message: &str) {
        if !crate::anthropic::subscription_oauth::is_subscription_auth_failure(error) {
            return;
        }
        let Some(path) = provider_auth_cooldown::record_at(
            self.provider_auth_cache_path().as_deref(),
            crate::anthropic::subscription_oauth::SUBSCRIPTION_AUTH_SCOPE,
            message,
            SystemTime::now(),
        ) else {
            return;
        };
        tracing::warn!(
            path = %path.display(),
            auth_scope = crate::anthropic::subscription_oauth::SUBSCRIPTION_AUTH_SCOPE,
            reason = "subscription-oauth",
            error = %message,
            "recorded Claude subscription OAuth cooldown; routing outer turns onto providers"
        );
    }

    pub(super) fn note_scoped_provider_exhaustion(
        &self,
        exhausted_model: Option<&str>,
        message: &str,
    ) {
        if !is_scoped_provider_exhaustion(message) {
            return;
        }
        let scopes = self.auth_scopes_for(exhausted_model, message);
        let cache_path = self.provider_auth_cache_path();
        let rate_limited = contains_rate_limit_marker(message);
        let quota_exhausted = contains_provider_quota_exhausted_marker(message);
        let reason = scoped_exhaustion_reason(message, rate_limited, quota_exhausted);
        for scope in scopes {
            record_scoped_exhaustion(
                cache_path.as_deref(),
                &scope,
                message,
                reason,
                rate_limited || quota_exhausted,
            );
        }
    }
}
