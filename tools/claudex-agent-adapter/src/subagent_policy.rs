use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use axum::http::HeaderMap;
use serde::Deserialize;

pub(crate) const ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS";
pub(crate) const CONFIG_ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG";
pub(crate) const RESOLVED_ENV_NAME: &str = "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS";
pub(crate) const HEADER_NAME: &str = "x-claudex-disabled-subagent-models";
const CONFIG_VERSION: u64 = 1;
const CONFIG_RELATIVE_PATH: &str = ".config/claudex/disabled-subagent-models.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ModelPolicy {
    version: u64,
    disabled_models: Vec<String>,
}

pub(crate) fn active_header() -> Result<Option<String>> {
    let path = config_path(
        std::env::var_os(CONFIG_ENV_NAME).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    merged_header(
        &load_config(path.as_deref())?,
        std::env::var_os(ENV_NAME).as_deref(),
    )
}

pub(crate) fn header_value(header: &Option<String>) -> &str {
    header.as_deref().unwrap_or_default()
}

pub(crate) fn apply_snapshot(command: &mut Command, header: &Option<String>) {
    command.env(RESOLVED_ENV_NAME, header_value(header));
}

fn config_path(explicit: Option<&OsStr>, home: Option<&OsStr>) -> Result<Option<PathBuf>> {
    if let Some(explicit) = explicit {
        if explicit.is_empty() {
            bail!("{CONFIG_ENV_NAME} must not be empty");
        }
        let path = PathBuf::from(explicit);
        if !path.is_file() {
            bail!("{CONFIG_ENV_NAME} does not name a readable file");
        }
        return Ok(Some(path));
    }
    Ok(home.map(|home| PathBuf::from(home).join(CONFIG_RELATIVE_PATH)))
}

fn load_config(path: Option<&Path>) -> Result<BTreeSet<String>> {
    let Some(path) = path.filter(|path| path.is_file()) else {
        return Ok(BTreeSet::new());
    };
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read disabled SubAgent model config {}", path.display()))?;
    let policy: ModelPolicy = serde_json::from_str(&contents)
        .with_context(|| format!("parse disabled SubAgent model config {}", path.display()))?;
    if policy.version != CONFIG_VERSION {
        bail!("disabled SubAgent model config version must be {CONFIG_VERSION}");
    }
    if policy.models_are_invalid() {
        bail!("disabledModels must contain unique, valid exact model IDs");
    }
    Ok(policy.disabled_models.into_iter().collect())
}

impl ModelPolicy {
    fn models_are_invalid(&self) -> bool {
        self.disabled_models
            .iter()
            .any(|model| !valid_model_id(model))
            || self.disabled_models.iter().collect::<BTreeSet<_>>().len()
                != self.disabled_models.len()
    }
}

pub(crate) fn merged_header(
    configured: &BTreeSet<String>,
    value: Option<&OsStr>,
) -> Result<Option<String>> {
    let mut models = configured.clone();
    if let Some(value) = value {
        models.extend(
            value
                .to_str()
                .context("CLAUDEX_DISABLED_SUBAGENT_MODELS must be valid UTF-8")
                .and_then(parse)?,
        );
    }
    Ok((!models.is_empty()).then(|| models.into_iter().collect::<Vec<_>>().join(",")))
}

pub(crate) fn request_models(headers: &HeaderMap) -> Result<BTreeSet<String>> {
    headers
        .get(HEADER_NAME)
        .map(|value| {
            value
                .to_str()
                .context("disabled SubAgent model header must be visible ASCII")
                .and_then(parse)
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse(value: &str) -> Result<BTreeSet<String>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| {
            if valid_model_id(model) {
                return Ok(model.to_owned());
            }
            bail!("{ENV_NAME} contains an invalid model ID")
        })
        .collect()
}

pub(crate) fn valid_model_id(model: &str) -> bool {
    !model.is_empty() && model.is_ascii() && model.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn merges_sorts_and_deduplicates_configured_and_environment_models() {
        let configured = BTreeSet::from(["gpt-5.6-sol".to_owned()]);
        assert_eq!(
            merged_header(
                &configured,
                Some(OsStr::new(" grok-4.5,gpt-5.6-sol,grok-4.5 "))
            )
            .expect("valid model policy"),
            Some("gpt-5.6-sol,grok-4.5".to_owned())
        );
        assert_eq!(
            merged_header(&configured, None).unwrap(),
            Some("gpt-5.6-sol".to_owned())
        );
        assert_eq!(merged_header(&BTreeSet::new(), None).unwrap(), None);
        assert_eq!(
            merged_header(&BTreeSet::new(), Some(OsStr::new(" , "))).unwrap(),
            None
        );
    }

    #[test]
    fn loads_dedicated_policy_and_resolves_terminal_specific_paths() {
        let root = tempfile::tempdir().unwrap();
        let default = root.path().join(CONFIG_RELATIVE_PATH);
        std::fs::create_dir_all(default.parent().unwrap()).unwrap();
        std::fs::write(
            &default,
            r#"{"version":1,"disabledModels":["grok-4.5","gpt-5.6-sol"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(None, Some(root.path().as_os_str())).unwrap(),
            Some(default.clone())
        );
        assert_eq!(
            load_config(Some(&default))
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "grok-4.5"]
        );

        let alternate = root.path().join("terminal.json");
        std::fs::write(
            &alternate,
            r#"{"version":1,"disabledModels":["qwen3.8-max-preview"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(Some(alternate.as_os_str()), None).unwrap(),
            Some(alternate)
        );
        assert!(config_path(Some(OsStr::new("")), None).is_err());
        assert!(config_path(Some(root.path().join("missing").as_os_str()), None).is_err());
        assert!(load_config(None).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_dedicated_policy_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("policy.json");
        for contents in [
            r#"{"version":2,"disabledModels":[]}"#,
            r#"{"version":1,"disabledModels":["invalid model"]}"#,
            r#"{"version":1,"disabledModels":["same","same"]}"#,
            r#"{"version":1,"disabledModels":[],"extra":true}"#,
            "not-json",
        ] {
            std::fs::write(&path, contents).unwrap();
            assert!(load_config(Some(&path)).is_err());
        }
    }

    #[test]
    fn reads_request_header_and_rejects_invalid_model_ids() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_NAME,
            "qwen3.8-max-preview,gpt-5.6-sol".parse().unwrap(),
        );
        assert_eq!(
            request_models(&headers)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "qwen3.8-max-preview"]
        );
        assert!(parse("model with spaces").is_err());
    }
}
