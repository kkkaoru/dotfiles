use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, Arc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Error, Result};
use axum::{body::Body, http::Response};

use super::subscription::failure::subscription_failure;
use super::{
    Bridge, MessagesRequest, provider_auth_cooldown, request_routing::RouteDecision, token_count,
    usage_limit_failover::UsageLimitFailover,
};

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

fn oauth_expiry_ms(value: &serde_json::Value, pointer: &str) -> Option<f64> {
    let expires_ms = value.pointer(pointer).and_then(|expires| {
        expires.as_f64().or_else(|| {
            expires
                .as_i64()
                .map(|millis| millis as f64)
                .or_else(|| expires.as_u64().map(|millis| millis as f64))
        })
    })?;
    if !expires_ms.is_finite() || expires_ms < 0.0 {
        return None;
    }
    Some(expires_ms)
}

fn expiry_ms_is_past(expires_ms: f64, now: SystemTime) -> bool {
    let expires = UNIX_EPOCH + Duration::from_millis(expires_ms as u64);
    expires <= now
}

pub(super) fn credentials_access_expired_at(
    credentials_path: &Path,
    now: SystemTime,
) -> Option<bool> {
    let value =
        serde_json::from_slice::<serde_json::Value>(&fs::read(credentials_path).ok()?).ok()?;
    Some(expiry_ms_is_past(
        oauth_expiry_ms(&value, "/claudeAiOauth/expiresAt")?,
        now,
    ))
}

/// Whether Claude subscription OAuth is known-dead from on-disk credentials.
///
/// Access-token `expiresAt` alone is not enough: Claude Code refreshes via
/// `refreshToken` while `refreshTokenExpiresAt` remains in the future. Treating
/// access expiry as unusable caused every outer Opus turn to preflight-failover
/// onto GPT even though `/login` was unnecessary.
pub(super) fn credentials_oauth_unusable_at(
    credentials_path: &Path,
    now: SystemTime,
) -> Option<bool> {
    let value =
        serde_json::from_slice::<serde_json::Value>(&fs::read(credentials_path).ok()?).ok()?;
    if let Some(refresh_ms) = oauth_expiry_ms(&value, "/claudeAiOauth/refreshTokenExpiresAt") {
        return Some(expiry_ms_is_past(refresh_ms, now));
    }
    match oauth_expiry_ms(&value, "/claudeAiOauth/expiresAt") {
        // Fresh access token: trust the file even if a prior cooldown remains.
        Some(access_ms) if !expiry_ms_is_past(access_ms, now) => Some(false),
        // Access expired without refresh lifetime: let Claude try a refresh, and
        // only stay away after a real auth-failure cooldown.
        Some(_) => None,
        None => None,
    }
}

fn default_credentials_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude/.credentials.json"))
}

/// Avoid spamming Claude Code logs when refresh is truly dead and every outer
/// turn still arrives as a native Opus subscription request.
fn warn_preflight_oauth_failover(exhausted_model: &str, failover_model: &str) {
    static LAST_WARN: Mutex<Option<Instant>> = Mutex::new(None);
    const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);
    let now = Instant::now();
    let emit = match LAST_WARN.lock() {
        Ok(mut guard) => match *guard {
            Some(previous) if now.duration_since(previous) < MIN_INTERVAL => false,
            _ => {
                *guard = Some(now);
                true
            }
        },
        Err(_) => true,
    };
    if emit {
        tracing::warn!(
            exhausted_model = %exhausted_model,
            failover_model = %failover_model,
            "preflight failover away from expired Claude subscription OAuth"
        );
    } else {
        tracing::debug!(
            exhausted_model = %exhausted_model,
            failover_model = %failover_model,
            "preflight failover away from expired Claude subscription OAuth (suppressed)"
        );
    }
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
            if !candidates.iter().any(|existing| existing == &model) {
                candidates.push(model);
            }
        }
        candidates.into_iter().find_map(|model| {
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
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "subscription_oauth_tests.rs"]
mod tests;
