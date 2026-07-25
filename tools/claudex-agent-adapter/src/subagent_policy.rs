use std::{collections::BTreeSet, ffi::OsStr};

use anyhow::{Context, Result, bail};
use axum::http::HeaderMap;

pub(crate) const ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS";
pub(crate) const HEADER_NAME: &str = "x-claudex-disabled-subagent-models";

pub(crate) fn current_environment_header() -> Result<Option<String>> {
    environment_header(std::env::var_os(ENV_NAME).as_deref())
}

pub(crate) fn environment_header(value: Option<&OsStr>) -> Result<Option<String>> {
    value
        .map(|value| {
            value
                .to_str()
                .context("CLAUDEX_DISABLED_SUBAGENT_MODELS must be valid UTF-8")
                .and_then(parse)
                .map(|models| {
                    (!models.is_empty()).then(|| models.into_iter().collect::<Vec<_>>().join(","))
                })
        })
        .transpose()
        .map(Option::flatten)
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
            if model.is_ascii() && model.bytes().all(|byte| byte.is_ascii_graphic()) {
                return Ok(model.to_owned());
            }
            bail!("{ENV_NAME} contains an invalid model ID")
        })
        .collect()
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn parses_sorts_and_deduplicates_environment_models() {
        assert_eq!(
            environment_header(Some(OsStr::new(" grok-4.5,gpt-5.6-sol,grok-4.5 ")))
                .expect("valid model policy"),
            Some("gpt-5.6-sol,grok-4.5".to_owned())
        );
        assert_eq!(environment_header(None).unwrap(), None);
        assert_eq!(environment_header(Some(OsStr::new(" , "))).unwrap(), None);
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
