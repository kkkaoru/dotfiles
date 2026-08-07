use anyhow::{Result, bail};
use serde_json::Value;

use super::{
    agent_effort::{is_agent_tool, requested_model},
    subscription::valid_effort,
};

const ADAPTER_EFFORT: &str = "claudex_effort";
const IMPLICIT_MODEL: &str = "claudex_implicit_model";

#[cfg(test)]
pub(super) fn validate_routed_agent_arguments(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
) -> Result<()> {
    validate_routed_agent_arguments_with_catalog(
        tool_name,
        arguments,
        user_messages,
        system,
        &crate::provider_config::ModelCatalog::default(),
    )
}

pub(super) fn validate_routed_agent_arguments_with_catalog(
    tool_name: &str,
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Result<()> {
    if !is_agent_tool(tool_name) {
        return Ok(());
    }
    let Some(model) = requested_model(arguments) else {
        bail!("{tool_name} launch is missing required `claudex_model`");
    };
    if !super::agent_routing::model_is_authorized_with_catalog(
        arguments,
        user_messages,
        system,
        model_catalog,
        model,
    ) {
        bail!(
            "{tool_name} launch model `{model}` does not match the exact route for its `subagent_type`"
        );
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
) -> Result<()> {
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
        bail!(
            "{tool_name} launch effort `{requested}` does not match `{expected}` for model `{model}`"
        );
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
