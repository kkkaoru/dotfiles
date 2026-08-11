use super::*;
use serde_json::json;

fn signature(system: &str, tools: Value, transport: Value) -> String {
    serde_json::to_string(&json!({
        "system": system,
        "tools": tools,
        "metadata": "client",
        "transport_identity": transport,
        "subagent_spawn_limit_reached": null,
        "working_directory": "/tmp/proj",
        "disabled_subagent_models": [],
        "advisor_model": null,
        "collaborator_model": null
    }))
    .expect("signature json")
}

#[test]
fn exact_strings_are_compatible_without_json() {
    assert!(signatures_compatible("plain", "plain"));
    assert!(!signatures_compatible("plain", "other"));
}

#[test]
fn system_text_drift_stays_compatible() {
    let tools = json!([{"name": "Bash"}]);
    let transport = json!({"session_id": "sess-1", "agent_id": null});
    assert!(signatures_compatible(
        &signature("old", tools.clone(), transport.clone()),
        &signature("new", tools, transport),
    ));
}

#[test]
fn main_session_id_without_agent_id_is_usable() {
    let tools = json!([]);
    let left = signature("s", tools.clone(), json!({"session_id": "sess-1"}));
    let right = signature("s2", tools, json!({"session_id": "sess-1"}));
    assert!(signatures_compatible(&left, &right));
}

#[test]
fn agent_id_without_session_id_is_usable() {
    let tools = json!([{"name": "Read"}]);
    let left = signature("s", tools.clone(), json!({"agent_id": "agent-a"}));
    let right = signature("s2", tools, json!({"agent_id": "agent-a"}));
    assert!(signatures_compatible(&left, &right));
}

#[test]
fn empty_transport_ids_are_not_a_stable_identity() {
    let tools = json!([]);
    let empty = signature(
        "s",
        tools.clone(),
        json!({"session_id": "", "agent_id": ""}),
    );
    let also_empty = signature("s2", tools, json!({"session_id": null, "agent_id": null}));
    assert!(!signatures_compatible(&empty, &also_empty));
    assert!(!usable_transport_identity(&json!(null)));
}

#[test]
fn unnamed_tools_disable_stable_identity() {
    let transport = json!({"session_id": "sess-1"});
    let named = signature("s", json!([{"name": "Bash"}]), transport.clone());
    let unnamed = signature("s", json!([{"description": "no name"}]), transport);
    assert!(!signatures_compatible(&named, &unnamed));
}
