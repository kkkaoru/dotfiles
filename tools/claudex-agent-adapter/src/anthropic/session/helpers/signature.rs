use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StableSignatureIdentity {
    transport_identity: Value,
    working_directory: Value,
    tool_names: Vec<String>,
    disabled_subagent_models: Value,
    advisor_model: Value,
    collaborator_model: Value,
    spawn_limit: Value,
    user_metadata: Value,
}

pub(super) fn stable_signature_identity(signature: &str) -> Option<StableSignatureIdentity> {
    let parsed: Value = serde_json::from_str(signature).ok()?;
    let object = parsed.as_object()?;
    let tools = object.get("tools")?.as_array()?;
    let mut tool_names: Vec<String> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    if !tools.is_empty() && tool_names.len() != tools.len() {
        return None;
    }
    let transport = object
        .get("transport_identity")
        .cloned()
        .unwrap_or(Value::Null);
    if !usable_transport_identity(&transport) {
        return None;
    }
    tool_names.sort_unstable();
    Some(StableSignatureIdentity {
        transport_identity: transport,
        working_directory: object
            .get("working_directory")
            .cloned()
            .unwrap_or(Value::Null),
        tool_names,
        disabled_subagent_models: object
            .get("disabled_subagent_models")
            .cloned()
            .unwrap_or(Value::Null),
        advisor_model: object.get("advisor_model").cloned().unwrap_or(Value::Null),
        collaborator_model: object
            .get("collaborator_model")
            .cloned()
            .unwrap_or(Value::Null),
        spawn_limit: object
            .get("subagent_spawn_limit_reached")
            .cloned()
            .unwrap_or(Value::Null),
        user_metadata: object.get("metadata").cloned().unwrap_or(Value::Null),
    })
}

pub(super) fn usable_transport_identity(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    ["session_id", "agent_id"].into_iter().any(|key| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    })
}
