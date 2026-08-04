use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WebSearchSettings {
    #[serde(default)]
    pub(super) fallback_providers: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
pub(super) struct RequestBudget {
    pub(super) estimated_requests: u64,
    pub(super) window_minutes: u64,
    pub(super) usage_window: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentChoice {
    pub(super) agent: String,
    pub(super) model: String,
    pub(super) effort: String,
}
