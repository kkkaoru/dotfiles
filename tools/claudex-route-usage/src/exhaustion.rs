//! Read adapter-written exhaustion cool-downs so routing can skip dead providers.
//!
//! `claudex-agent-adapter` records rate-limit / auth failures under
//! `~/.cache/claudex/provider-auth-cooldown.json` (model and provider scopes)
//! and classic Codex usage limits under `codex-app-server-usage-limit.json`.
//! Without consulting those files, CodexBar can keep ranking a model that the
//! adapter already fail-fasts, which produces 502 retry storms.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;

const AUTH_COOLDOWN_FILE: &str = "provider-auth-cooldown.json";
const USAGE_LIMIT_FILE: &str = "codex-app-server-usage-limit.json";
const CACHE_VERSION: u64 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveState {
    pub scopes: BTreeSet<String>,
    pub codex_backend_cooling: bool,
}

pub fn live_state(home: impl AsRef<Path>, now: SystemTime) -> LiveState {
    let home = home.as_ref();
    LiveState {
        scopes: active_scopes(home, now),
        codex_backend_cooling: codex_app_server_cooling_down(home, now),
    }
}

/// Union live adapter cooldowns into the terminal model denylist.
///
/// The resulting set participates in the routing cache key. A cooldown start
/// or expiry therefore invalidates an otherwise matching quota snapshot before
/// it can be served by the hook.
pub fn effective_disabled_models(
    config: &Config,
    configured: &BTreeSet<String>,
    state: &LiveState,
) -> BTreeSet<String> {
    let mut disabled = configured.clone();
    for provider in &config.providers {
        if provider_is_exhausted(provider, &state.scopes, state.codex_backend_cooling)
            && let Some(model) = provider_model(provider)
        {
            disabled.insert(model.to_owned());
        }
    }
    for worker in &config.native_workers {
        let model = worker.get("model").and_then(Value::as_str);
        let scoped = model.is_some_and(|value| state.scopes.contains(value))
            || ["usageProvider", "modelProvider"].iter().any(|key| {
                worker
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| state.scopes.contains(value))
            });
        if scoped && let Some(model) = model {
            disabled.insert(model.to_owned());
        }
    }
    if let Some(model) = config.fallback.get("model").and_then(Value::as_str)
        && state.scopes.contains(model)
    {
        disabled.insert(model.to_owned());
    }
    disabled
}

fn provider_model(provider: &Value) -> Option<&str> {
    provider
        .get("subagentModel")
        .and_then(Value::as_str)
        .or_else(|| provider.get("defaultModel").and_then(Value::as_str))
        .filter(|model| !model.is_empty())
}

/// Active exhaustion scopes (exact model ids and usage/model providers).
pub fn active_scopes(home: impl AsRef<Path>, now: SystemTime) -> BTreeSet<String> {
    let now = unix_seconds(now);
    let mut scopes = BTreeSet::new();
    let path = home
        .as_ref()
        .join(".cache/claudex")
        .join(AUTH_COOLDOWN_FILE);
    let Some(cache) = read_json(&path) else {
        return scopes;
    };
    if cache.get("version").and_then(Value::as_u64) != Some(CACHE_VERSION) {
        return scopes;
    }
    let Some(entries) = cache.get("entries").and_then(Value::as_object) else {
        return scopes;
    };
    for (scope, entry) in entries {
        let Some(until) = entry.get("untilUnixSeconds").and_then(Value::as_u64) else {
            continue;
        };
        if now < until {
            scopes.insert(scope.clone());
        }
    }
    scopes
}

/// True when the Codex app-server backend itself is cooling down.
pub fn codex_app_server_cooling_down(home: impl AsRef<Path>, now: SystemTime) -> bool {
    let path = home.as_ref().join(".cache/claudex").join(USAGE_LIMIT_FILE);
    let Some(cooldown) = read_json(&path) else {
        return false;
    };
    cooldown.get("version").and_then(Value::as_u64) == Some(CACHE_VERSION)
        && cooldown.get("backend").and_then(Value::as_str) == Some("codex-app-server")
        && cooldown
            .get("untilUnixSeconds")
            .and_then(Value::as_u64)
            .is_some_and(|until| unix_seconds(now) < until)
}

/// Whether one configured provider should leave automatic selection this turn.
pub fn provider_is_exhausted(
    provider: &Value,
    scopes: &BTreeSet<String>,
    codex_backend_cooling: bool,
) -> bool {
    if scopes.is_empty() && !codex_backend_cooling {
        return false;
    }
    let model = provider
        .get("subagentModel")
        .and_then(Value::as_str)
        .or_else(|| provider.get("defaultModel").and_then(Value::as_str))
        .unwrap_or_default();
    if !model.is_empty() && scopes.contains(model) {
        return true;
    }
    for key in ["usageProvider", "modelProvider"] {
        if let Some(scope) = provider.get(key).and_then(Value::as_str)
            && !scope.is_empty()
            && scopes.contains(scope)
        {
            return true;
        }
    }
    if codex_backend_cooling {
        return provider
            .get("backend")
            .and_then(Value::as_str)
            .is_some_and(|backend| backend == "codex-app-server");
    }
    false
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use serde_json::json;
    use std::time::Duration;

    fn routing_config() -> Config {
        let provider = json!({
            "id": "ollama-glm",
            "agent": "claudex-ollama",
            "defaultModel": "glm-5.2:cloud",
            "effort": "high",
            "backend": "ollama",
            "usageProvider": "ollama"
        });
        let fallback = json!({
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
        });
        let raw = json!({
            "version": 1,
            "providers": [provider],
            "fallback": fallback,
            "nativeWorkers": []
        });
        Config {
            raw,
            providers: vec![provider],
            native_workers: Vec::new(),
            fallback,
            advisor: config::default_advisor(),
        }
    }

    #[test]
    fn reads_active_model_and_provider_scopes() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join(".cache/claudex");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join(AUTH_COOLDOWN_FILE),
            r#"{
              "version": 1,
              "entries": {
                "glm-5.2:cloud": {"untilUnixSeconds": 2000, "message": "429", "recordedUnixSeconds": 1000},
                "ollama": {"untilUnixSeconds": 2000, "message": "429", "recordedUnixSeconds": 1000},
                "expired": {"untilUnixSeconds": 500, "message": "old", "recordedUnixSeconds": 100}
              }
            }"#,
        )
        .unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let scopes = active_scopes(root.path(), now);
        assert!(scopes.contains("glm-5.2:cloud"));
        assert!(scopes.contains("ollama"));
        assert!(!scopes.contains("expired"));
    }

    #[test]
    fn marks_provider_exhausted_by_model_or_usage_provider() {
        let scopes = BTreeSet::from(["ollama".to_owned()]);
        let glm = json!({
            "id": "ollama-glm-5-2",
            "defaultModel": "glm-5.2:cloud",
            "usageProvider": "ollama",
            "backend": "codex-app-server"
        });
        let grok = json!({
            "id": "grok",
            "defaultModel": "grok-4.5",
            "usageProvider": "grok",
            "backend": "grok-acp"
        });
        assert!(provider_is_exhausted(&glm, &scopes, false));
        assert!(!provider_is_exhausted(&grok, &scopes, false));
        assert!(provider_is_exhausted(&glm, &BTreeSet::new(), true));
        assert!(!provider_is_exhausted(&grok, &BTreeSet::new(), true));
    }

    #[test]
    fn detects_active_codex_app_server_usage_limit_cooldown() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join(".cache/claudex");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            cache.join(USAGE_LIMIT_FILE),
            r#"{
              "version": 1,
              "backend": "codex-app-server",
              "untilUnixSeconds": 2000,
              "message": "usage limit"
            }"#,
        )
        .unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(codex_app_server_cooling_down(root.path(), now));
        assert!(!codex_app_server_cooling_down(
            root.path(),
            UNIX_EPOCH + Duration::from_secs(3_000)
        ));
    }

    #[test]
    fn cooldown_start_and_expiry_change_the_snapshot_policy_key() {
        let config = routing_config();
        let configured = BTreeSet::new();
        let clear = effective_disabled_models(&config, &configured, &LiveState::default());
        let active = effective_disabled_models(
            &config,
            &configured,
            &LiveState {
                scopes: BTreeSet::from(["ollama".to_owned()]),
                codex_backend_cooling: false,
            },
        );

        assert!(clear.is_empty());
        assert_eq!(active, BTreeSet::from(["glm-5.2:cloud".to_owned()]));
        assert_ne!(
            config::configuration_key(&config.raw, &clear),
            config::configuration_key(&config.raw, &active)
        );
        assert_eq!(
            effective_disabled_models(&config, &configured, &LiveState::default()),
            clear,
            "an expired cooldown restores the original snapshot generation"
        );
    }
}
