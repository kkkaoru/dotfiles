use super::{ADAPTER_MODEL, IMPLICIT_MODEL, hydrate_standard_agent_to_parent};
use crate::provider_config::{ModelCatalog, WorkerRoute};

fn gpt_luna_catalog() -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            WorkerRoute::new("claudex-gpt", "gpt-5.6-luna", "max"),
            WorkerRoute::new("claudex-opencode-gpt", "opencode-go/gpt-5.6-luna", "max"),
        ])
        .expect("luna workers");
    catalog
}

#[test]
fn standard_agent_without_type_does_not_inherit_parent_model() {
    let mut arguments = serde_json::json!({"prompt": "inspect the current state"});

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &gpt_luna_catalog());

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn configured_worker_type_without_model_is_not_guessed() {
    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-gpt",
        "prompt": "inspect the current state"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &gpt_luna_catalog());

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn generic_nested_from_gpt_luna_does_not_keep_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "claudex_effort": "xhigh",
        "prompt": "research the catalog"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "gpt-5.6-luna");
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn generic_nested_from_opencode_luna_does_not_keep_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "Explore",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "prompt": "explore"
    });

    hydrate_standard_agent_to_parent(
        &mut arguments,
        "opencode-go/gpt-5.6-luna",
        &gpt_luna_catalog(),
    );

    assert_eq!(arguments[ADAPTER_MODEL], "opencode-go/gpt-5.6-luna");
}

#[test]
fn generic_nested_without_catalog_does_not_infer_luna_from_model_string() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "prompt": "research"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &ModelCatalog::default());

    assert_eq!(arguments[ADAPTER_MODEL], "cline-pass/deepseek-v4-flash");
}

#[test]
fn generic_nested_from_gpt_luna_keeps_non_cline_child() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "qwen3.8-max-preview",
        "prompt": "research"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "qwen3.8-max-preview");
}

#[test]
fn explicit_cline_worker_from_gpt_luna_parent_stays_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-cline-deepseek-flash",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "claudex_effort": "xhigh",
        "prompt": "explicit cline launch"
    });

    hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna", &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "cline-pass/deepseek-v4-flash");
}
