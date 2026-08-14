use serde_json::Value;
use std::fmt;

use super::{
    agent_effort::{is_agent_tool, requested_model},
    subscription::valid_effort,
};

pub(in crate::anthropic) const BLOCKED_SUBAGENT_NOTICE: &str =
    "The requested SubAgent model is not configured, so it was not started. Continue without it.";

const CATALOG_RESOLUTION_NOTICE: &str = "The requested SubAgent route could not be resolved from the active model catalog, so it was not started. Continue without it.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::anthropic) enum BlockedSubagentReason {
    MissingConfig,
    PolicyDisabled,
    Cooldown,
    CatalogResolution,
}

impl BlockedSubagentReason {
    pub(super) fn notice(self, model: Option<&str>) -> String {
        match self {
            Self::MissingConfig => BLOCKED_SUBAGENT_NOTICE.to_owned(),
            Self::PolicyDisabled => model.map_or_else(
                || {
                    "The requested SubAgent model is disabled by policy, so it was not started. Continue without it."
                        .to_owned()
                },
                |model| {
                    format!("SubAgent model `{model}` is disabled by policy and was not launched.")
                },
            ),
            Self::Cooldown => model.map_or_else(
                || {
                    "The requested SubAgent provider is cooling down after a rate/usage/billing limit; pick another selected_workers entry."
                        .to_owned()
                },
                |model| {
                    format!(
                        "SubAgent model `{model}` is cooling down after a rate/usage/billing limit; pick another selected_workers entry."
                    )
                },
            ),
            Self::CatalogResolution => model.map_or_else(
                || CATALOG_RESOLUTION_NOTICE.to_owned(),
                |model| {
                    format!(
                        "SubAgent route for `{model}` could not be resolved from the active model catalog, so it was not started. Continue without it."
                    )
                },
            ),
        }
    }
}

#[derive(Debug)]
pub(in crate::anthropic) struct BlockedSubagentError {
    reason: BlockedSubagentReason,
    model: Option<String>,
    detail: String,
}

impl BlockedSubagentError {
    pub(super) fn missing_config(detail: impl Into<String>) -> Self {
        Self::new(BlockedSubagentReason::MissingConfig, None, detail)
    }

    pub(super) fn policy_disabled(model: &str) -> Self {
        Self::new(
            BlockedSubagentReason::PolicyDisabled,
            Some(model.to_owned()),
            format!(
                "SubAgent model `{model}` is disabled by the active Claudex policy and must not be launched"
            ),
        )
    }

    pub(super) fn cooldown(model: &str) -> Self {
        Self::new(
            BlockedSubagentReason::Cooldown,
            Some(model.to_owned()),
            Self::cooldown_detail(model),
        )
    }

    pub(super) fn catalog_resolution(model: &str, detail: impl Into<String>) -> Self {
        Self::new(
            BlockedSubagentReason::CatalogResolution,
            Some(model.to_owned()),
            detail,
        )
    }

    #[cfg(test)]
    pub(super) fn reason(&self) -> BlockedSubagentReason {
        self.reason
    }

    pub(super) fn notice(&self) -> String {
        self.reason.notice(self.model.as_deref())
    }

    fn new(
        reason: BlockedSubagentReason,
        model: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            model,
            detail: detail.into(),
        }
    }

    fn cooldown_detail(model: &str) -> String {
        format!(
            "SubAgent model `{model}` is cooling down after a rate/usage/billing limit; pick another selected_workers entry."
        )
    }
}

impl fmt::Display for BlockedSubagentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for BlockedSubagentError {}

const ADAPTER_EFFORT: &str = "claudex_effort";
const IMPLICIT_MODEL: &str = "claudex_implicit_model";

#[cfg(test)]
pub(super) fn validate_routed_agent_arguments(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
) -> anyhow::Result<()> {
    validate_routed_agent_arguments_with_reason(
        tool_name,
        arguments,
        user_messages,
        system,
        &crate::provider_config::ModelCatalog::default(),
    )
    .map_err(anyhow::Error::new)
}

#[cfg(test)]
pub(super) fn validate_routed_agent_arguments_with_catalog(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> anyhow::Result<()> {
    validate_routed_agent_arguments_with_reason(
        tool_name,
        arguments,
        user_messages,
        system,
        model_catalog,
    )
    .map_err(anyhow::Error::new)
}

pub(super) fn validate_routed_agent_arguments_with_reason(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> std::result::Result<(), BlockedSubagentError> {
    if !is_agent_tool(tool_name) {
        return Ok(());
    }
    let Some(model) = requested_model(arguments) else {
        return Err(BlockedSubagentError::missing_config(format!(
            "{tool_name} launch is missing required `claudex_model`"
        )));
    };
    if !super::agent_routing::model_is_authorized_with_catalog(
        arguments,
        user_messages,
        system,
        model_catalog,
        model,
    ) {
        return Err(BlockedSubagentError::catalog_resolution(
            model,
            format!(
                "{tool_name} launch model `{model}` does not match the exact route for its `subagent_type`"
            ),
        ));
    }
    validate_effort(
        tool_name,
        arguments,
        user_messages,
        system,
        model_catalog,
        model,
    )
}

fn validate_effort(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> std::result::Result<(), BlockedSubagentError> {
    let requested = arguments
        .get(ADAPTER_EFFORT)
        .or_else(|| arguments.get("effort"))
        .and_then(Value::as_str)
        .map(|effort| if effort == "mid" { "medium" } else { effort })
        .filter(|effort| valid_effort(effort));
    let expected = super::agent_routing::expected_worker_fields(
        arguments,
        user_messages,
        system,
        model_catalog,
    )
    .and_then(|(_, effort)| effort)
    .or_else(|| implicit_model_effort(arguments, model_catalog, model));
    if let (Some(expected), Some(requested)) = (expected, requested)
        && requested != expected
    {
        return Err(BlockedSubagentError::catalog_resolution(
            model,
            format!(
                "{tool_name} launch effort `{requested}` does not match `{expected}` for model `{model}`"
            ),
        ));
    }
    Ok(())
}

fn implicit_model_effort(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> Option<String> {
    (arguments.get(IMPLICIT_MODEL).and_then(Value::as_str) == Some(model))
        .then(|| model_catalog.worker_effort_for_model(model))
        .flatten()
        .map(str::to_owned)
}
