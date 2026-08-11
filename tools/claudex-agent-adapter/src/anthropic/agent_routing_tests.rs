use super::{ADAPTER_MODEL, IMPLICIT_MODEL, hydrate_standard_agent_to_parent};

#[test]
fn standard_agent_without_type_does_not_inherit_parent_model() {
    let mut arguments = serde_json::json!({"prompt": "inspect the current state"});

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna");

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn configured_worker_type_without_model_is_not_guessed() {
    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-gpt",
        "prompt": "inspect the current state"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna");

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}
