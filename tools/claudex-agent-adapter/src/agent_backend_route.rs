use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::{BackendKind, WebSearchMode};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcpLaunch {
    pub program: String,
    pub arguments: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BackendRoute {
    pub model: String,
    pub backend: BackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pi_extensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_prefixes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp: Option<AcpLaunch>,
    #[serde(default, skip_serializing_if = "WebSearchMode::is_default")]
    pub web_search_mode: WebSearchMode,
}
impl BackendRoute {
    pub fn new(model: impl Into<String>, backend: BackendKind) -> Self {
        Self {
            model: model.into(),
            backend,
            effort: None,
            model_provider: None,
            model_catalog_json: None,
            pi_provider: None,
            pi_model: None,
            pi_extensions: Vec::new(),
            max_context_tokens: None,
            max_concurrency: None,
            model_prefixes: Vec::new(),
            acp: None,
            web_search_mode: WebSearchMode::default(),
        }
    }
    pub fn description(&self) -> String {
        if self.model_provider.is_none()
            && self.effort.is_none()
            && self.model_catalog_json.is_none()
            && self.pi_provider.is_none()
            && self.pi_model.is_none()
            && self.pi_extensions.is_empty()
            && self.max_context_tokens.is_none()
            && self.max_concurrency.is_none()
            && self.model_prefixes.is_empty()
            && self.acp.is_none()
            && self.web_search_mode.is_default()
        {
            return format!("{}={}", self.model, self.backend);
        }
        serde_json::to_string(self).expect("backend route must serialize")
    }
}
impl FromStr for BackendRoute {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let (model, backend) = value
            .split_once('=')
            .context("--backend-route must use MODEL=BACKEND")?;
        if model.is_empty() {
            bail!("--backend-route model must not be empty");
        }
        Ok(Self::new(model, backend.parse()?))
    }
}
