//! Hostname denylist loading and agreement with `providers.json` `enabled`.

use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::Path;

use super::valid_model_id;

/// Distinguishes a missing policy, a live parse, last-known-good, and cold-start failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisabledModelsPolicy {
    /// Optional denylist file is absent. Empty is intentional, not allow-all-from-failure.
    Unset,
    Set(BTreeSet<String>),
    /// Read failed; keep last successfully parsed models (including an explicit empty set).
    Stale {
        models: BTreeSet<String>,
        message: String,
    },
    /// Corrupt/unreadable file and no last-known-good. Fail-closed: do not treat as empty allow-all.
    Unavailable {
        message: String,
    },
}

impl DisabledModelsPolicy {
    pub fn models(&self) -> BTreeSet<String> {
        match self {
            Self::Set(models) | Self::Stale { models, .. } => models.clone(),
            Self::Unset | Self::Unavailable { .. } => BTreeSet::new(),
        }
    }

    pub fn load_error(&self) -> Option<&str> {
        match self {
            Self::Stale { message, .. } | Self::Unavailable { message } => Some(message),
            Self::Unset | Self::Set(_) => None,
        }
    }

    pub fn source(&self) -> &'static str {
        match self {
            Self::Set(_) => "live",
            Self::Unset => "unset",
            Self::Stale { .. } => "last-known-good",
            Self::Unavailable { .. } => "unavailable",
        }
    }
}

fn with_last_good<R>(update: impl FnOnce(&mut Option<BTreeSet<String>>) -> R) -> R {
    #[cfg(test)]
    {
        thread_local! {
            static LAST_GOOD: std::cell::RefCell<Option<BTreeSet<String>>> =
                const { std::cell::RefCell::new(None) };
        }
        LAST_GOOD.with(|slot| update(&mut slot.borrow_mut()))
    }
    #[cfg(not(test))]
    {
        static LAST_GOOD: std::sync::Mutex<Option<BTreeSet<String>>> = std::sync::Mutex::new(None);
        let mut slot = LAST_GOOD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut slot)
    }
}

pub fn load_disabled_models_policy(path: &Path) -> DisabledModelsPolicy {
    match load_disabled_models_strict(path) {
        Ok(None) => missing_denylist(path),
        Ok(Some(models)) if models.is_empty() => {
            with_last_good(|slot| *slot = Some(BTreeSet::new()));
            DisabledModelsPolicy::Unset
        }
        Ok(Some(models)) => {
            with_last_good(|slot| *slot = Some(models.clone()));
            DisabledModelsPolicy::Set(models)
        }
        Err(error) => fallback_read_failure(path, error),
    }
}

fn missing_denylist(path: &Path) -> DisabledModelsPolicy {
    match with_last_good(|slot| slot.clone()) {
        Some(models) => DisabledModelsPolicy::Stale {
            message: format!(
                "denylist file missing; using last-known-good for {}",
                path.display()
            ),
            models,
        },
        None => DisabledModelsPolicy::Unset,
    }
}

fn fallback_read_failure(path: &Path, error: anyhow::Error) -> DisabledModelsPolicy {
    match with_last_good(|slot| slot.clone()) {
        Some(models) => DisabledModelsPolicy::Stale {
            message: format!(
                "{error}; using last-known-good denylist for {}",
                path.display()
            ),
            models,
        },
        None => DisabledModelsPolicy::Unavailable {
            message: format!(
                "{error}; denylist unavailable at cold start for {}",
                path.display()
            ),
        },
    }
}

fn load_disabled_models_strict(path: &Path) -> Result<Option<BTreeSet<String>>> {
    let text = match std::fs::read_to_string(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(text) if text.trim().is_empty() => return Ok(None),
        Ok(text) => text,
    };
    Ok(Some(parse_disabled_models(&text)?))
}

fn parse_disabled_models(text: &str) -> Result<BTreeSet<String>> {
    let policy: Value = serde_json::from_str(text)?;
    let Some(object) = policy.as_object() else {
        bail!("disabled SubAgent model config must use version 1 schema");
    };
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    if keys != BTreeSet::from(["version", "disabledModels"])
        || object.get("version").and_then(Value::as_i64) != Some(1)
    {
        bail!("disabled SubAgent model config must use version 1 schema");
    }
    let Some(models) = object.get("disabledModels").and_then(Value::as_array) else {
        bail!("disabledModels must contain valid exact model IDs");
    };
    if !models.iter().all(valid_model_id) {
        bail!("disabledModels must contain valid exact model IDs");
    }
    let set: BTreeSet<String> = models
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::to_owned)
        .collect();
    if set.len() != models.len() {
        bail!("disabledModels must not contain duplicates");
    }
    Ok(set)
}

#[cfg(test)]
pub fn provider_models(provider: &Value) -> Vec<String> {
    let default = provider.get("defaultModel").and_then(Value::as_str);
    let subagent = provider.get("subagentModel").and_then(Value::as_str);
    default
        .into_iter()
        .chain(subagent)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
pub fn provider_enabled_flag(provider: &Value) -> bool {
    provider
        .get("enabled")
        .is_none_or(|value| value.as_bool().unwrap_or(false))
}

#[cfg(test)]
pub fn provider_effectively_enabled(provider: &Value, denylist: &BTreeSet<String>) -> bool {
    provider_enabled_flag(provider)
        && !provider_models(provider)
            .iter()
            .any(|model| denylist.contains(model))
}

/// Provider ids that claim `enabled: true` while listing a hostname-denylisted model.
#[cfg(test)]
pub fn enabled_denylist_conflicts<'a>(
    providers: impl IntoIterator<Item = &'a Value>,
    denylist: &BTreeSet<String>,
) -> Vec<String> {
    providers
        .into_iter()
        .filter(|provider| provider_enabled_flag(provider))
        .filter(|provider| {
            provider_models(provider)
                .iter()
                .any(|model| denylist.contains(model))
        })
        .filter_map(|provider| {
            provider
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DisabledModelsPolicy, enabled_denylist_conflicts, load_disabled_models_policy,
        provider_effectively_enabled, with_last_good,
    };
    use crate::config::load_disabled_models;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn clear_last_good() {
        with_last_good(|slot| *slot = None);
    }

    #[test]
    fn missing_denylist_file_without_last_good_is_optional_unset() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing-disabled-subagent-models.json");
        let policy = load_disabled_models_policy(&path);
        assert_eq!(policy, DisabledModelsPolicy::Unset);
        assert!(policy.models().is_empty());
        assert!(policy.load_error().is_none());
        assert_eq!(policy.source(), "unset");
    }

    #[test]
    fn empty_denylist_file_is_unset_not_uninitialized() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, r#"{"version":1,"disabledModels":[]}"#).unwrap();
        assert_eq!(
            load_disabled_models_policy(&path),
            DisabledModelsPolicy::Unset
        );
    }

    #[test]
    fn blank_denylist_file_without_last_good_is_optional_unset() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, " \n").unwrap();
        let policy = load_disabled_models_policy(&path);
        assert_eq!(policy, DisabledModelsPolicy::Unset);
        assert!(policy.models().is_empty());
        assert!(policy.load_error().is_none());
        assert!(load_disabled_models(&path).unwrap().is_empty());
    }

    #[test]
    fn corrupt_denylist_without_last_good_is_fail_closed() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, "not-json").unwrap();
        let policy = load_disabled_models_policy(&path);
        assert!(
            matches!(policy, DisabledModelsPolicy::Unavailable { .. }),
            "{policy:?}"
        );
        assert!(policy.load_error().is_some());
        assert_eq!(policy.source(), "unavailable");
        let error = load_disabled_models(&path).expect_err("cold start must not allow-all");
        assert!(
            error
                .to_string()
                .contains("denylist unavailable at cold start"),
            "{error}"
        );
    }

    #[test]
    fn read_failure_keeps_last_known_good_instead_of_empty() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(
            &path,
            r#"{"version":1,"disabledModels":["opencode-go/deepseek-v4-flash"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_disabled_models_policy(&path).models(),
            BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()])
        );
        std::fs::write(&path, "not-json").unwrap();
        let policy = load_disabled_models_policy(&path);
        match policy {
            DisabledModelsPolicy::Stale { models, message } => {
                assert_eq!(
                    models,
                    BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()])
                );
                assert!(message.contains("last-known-good"));
            }
            other => panic!("expected stale last-known-good, got {other:?}"),
        }
        assert_eq!(
            load_disabled_models(&path).unwrap(),
            BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()])
        );
    }

    #[test]
    fn hostname_denylist_overrides_providers_json_enabled() {
        let providers = json!([
            {
                "id": "opencode-go",
                "defaultModel": "opencode-go/deepseek-v4-flash",
                "enabled": true
            },
            {
                "id": "codex",
                "defaultModel": "gpt-5.6-luna",
                "enabled": true
            },
            {
                "id": "off",
                "defaultModel": "denied-other",
                "enabled": false
            }
        ]);
        let denylist = BTreeSet::from([
            "opencode-go/deepseek-v4-flash".to_owned(),
            "denied-other".to_owned(),
        ]);
        let list = providers.as_array().unwrap();
        assert_eq!(
            enabled_denylist_conflicts(list, &denylist),
            ["opencode-go".to_owned()]
        );
        assert!(!provider_effectively_enabled(&list[0], &denylist));
        assert!(provider_effectively_enabled(&list[1], &denylist));
        assert!(!provider_effectively_enabled(&list[2], &denylist));
    }
}
