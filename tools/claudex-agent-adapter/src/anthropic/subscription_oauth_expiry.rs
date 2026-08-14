use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) fn oauth_expiry_ms(value: &serde_json::Value, pointer: &str) -> Option<f64> {
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

pub(super) fn expiry_ms_is_past(expires_ms: f64, now: SystemTime) -> bool {
    let expires = UNIX_EPOCH + Duration::from_millis(expires_ms as u64);
    expires <= now
}

#[cfg(test)]
pub(in crate::anthropic) fn credentials_access_expired_at(
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
pub(in crate::anthropic) fn credentials_oauth_unusable_at(
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

pub(super) fn default_credentials_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude/.credentials.json"))
}

pub(super) fn push_unique(candidates: &mut Vec<String>, model: String) {
    if candidates.iter().any(|existing| existing == &model) {
        return;
    }
    candidates.push(model);
}

/// Avoid spamming Claude Code logs when refresh is truly dead and every outer
/// turn still arrives as a native Opus subscription request.
pub(super) fn warn_preflight_oauth_failover(exhausted_model: &str, failover_model: &str) {
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
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expiry_ms_rejects_invalid() {
        use serde_json::json;
        let v = json!({"bad": "str"});
        assert_eq!(oauth_expiry_ms(&v, "/bad"), None);
    }

    #[test]
    fn expiry_boundary() {
        let now = UNIX_EPOCH + Duration::from_millis(1000);
        assert!(expiry_ms_is_past(1000.0, now));
    }

    #[test]
    fn oauth_expiry_accepts_unsigned_values_above_i64_range() {
        let value = json!({"expiresAt": (i64::MAX as u64) + 1});
        let expiry = oauth_expiry_ms(&value, "/expiresAt");
        assert_eq!(expiry, Some(((i64::MAX as u64) + 1) as f64));
    }

    #[test]
    fn expired_access_without_refresh_lifetime_is_unknown() {
        let root = tempfile::tempdir().expect("oauth credentials fixture");
        let path = root.path().join("credentials.json");
        std::fs::write(&path, r#"{"claudeAiOauth":{"expiresAt":1}}"#).expect("write credentials");

        assert_eq!(
            credentials_oauth_unusable_at(&path, UNIX_EPOCH + Duration::from_secs(2)),
            None,
            "an expired access token without a refresh lifetime must remain retryable"
        );
    }

    #[test]
    fn push_unique_deduplicates_existing_fallback_before_appending() {
        let mut candidates = vec!["fallback".to_owned()];
        push_unique(&mut candidates, "fallback".to_owned());
        push_unique(&mut candidates, "other-fallback".to_owned());

        assert_eq!(
            candidates,
            vec!["fallback".to_owned(), "other-fallback".to_owned()]
        );
    }
}
