//! Hostname denylist loading and agreement with `providers.json` `enabled`.

use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

use super::valid_model_id;

/// Distinguishes a missing/unreadable policy from an explicit empty denylist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisabledModelsPolicy {
    Uninitialized,
    Unset,
    Set(BTreeSet<String>),
}

impl DisabledModelsPolicy {
    pub fn models(&self) -> BTreeSet<String> {
        match self {
            Self::Set(models) => models.clone(),
            Self::Uninitialized | Self::Unset => BTreeSet::new(),
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
        Ok(models) if models.is_empty() => {
            with_last_good(|slot| *slot = Some(BTreeSet::new()));
            DisabledModelsPolicy::Unset
        }
        Ok(models) => {
            with_last_good(|slot| *slot = Some(models.clone()));
            DisabledModelsPolicy::Set(models)
        }
        Err(error) => fail_open_disabled_models(path, error),
    }
}

fn fail_open_disabled_models(path: &Path, error: anyhow::Error) -> DisabledModelsPolicy {
    if let Some(models) = with_last_good(|slot| slot.clone()) {
        eprintln!(
            "claudex: {error}; using last valid disabled SubAgent model config for {}",
            path.display()
        );
        return if models.is_empty() {
            DisabledModelsPolicy::Unset
        } else {
            DisabledModelsPolicy::Set(models)
        };
    }
    eprintln!(
        "claudex: {error}; denylist uninitialized, fail-open with empty policy for {}",
        path.display()
    );
    DisabledModelsPolicy::Uninitialized
}

fn load_disabled_models_strict(path: &Path) -> Result<BTreeSet<String>> {
    let text = std::fs::read_to_string(path)?;
    let policy: Value = serde_json::from_str(&text)?;
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
        provider_effectively_enabled,
    };
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn missing_denylist_file_is_uninitialized_fail_open() {
        let policy = load_disabled_models_policy(std::path::Path::new(
            "/tmp/claudex-missing-denylist-uninitialized.json",
        ));
        assert_eq!(policy, DisabledModelsPolicy::Uninitialized);
        assert!(policy.models().is_empty());
    }

    #[test]
    fn empty_denylist_file_is_unset_not_uninitialized() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, r#"{"version":1,"disabledModels":[]}"#).unwrap();
        assert_eq!(
            load_disabled_models_policy(&path),
            DisabledModelsPolicy::Unset
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
