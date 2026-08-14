use super::{ADAPTER_MODEL, IMPLICIT_MODEL, hydrate_standard_agent_to_parent};
use crate::provider_config::{ModelCatalog, WorkerRoute};

const CODEX_LUNA: &str = "codex-luna-from-catalog";
const OPENCODE_LUNA: &str = "opencode-go/catalog-luna";

fn gpt_luna_catalog() -> ModelCatalog {
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            WorkerRoute::new("claudex-gpt", CODEX_LUNA, "max"),
            WorkerRoute::new("claudex-opencode-gpt", OPENCODE_LUNA, "max"),
        ])
        .expect("luna workers");
    catalog
}

#[test]
fn standard_agent_without_type_does_not_inherit_parent_model() {
    let mut arguments = serde_json::json!({"prompt": "inspect the current state"});

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &gpt_luna_catalog());

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn configured_worker_type_without_model_is_not_guessed() {
    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-gpt",
        "prompt": "inspect the current state"
    });

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &gpt_luna_catalog());

    assert!(arguments.get(ADAPTER_MODEL).is_none());
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn generic_nested_from_codex_luna_does_not_keep_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "claudex_effort": "xhigh",
        "prompt": "research the catalog"
    });

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], CODEX_LUNA);
    assert!(arguments.get(IMPLICIT_MODEL).is_none());
}

#[test]
fn generic_nested_from_opencode_luna_keeps_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "Explore",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "prompt": "explore"
    });

    hydrate_standard_agent_to_parent(&mut arguments, OPENCODE_LUNA, &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "cline-pass/deepseek-v4-flash");
}

#[test]
fn generic_nested_without_catalog_does_not_infer_luna_from_model_string() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "prompt": "research"
    });

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &ModelCatalog::default());

    assert_eq!(arguments[ADAPTER_MODEL], "cline-pass/deepseek-v4-flash");
}

#[test]
fn generic_nested_from_codex_luna_keeps_non_cline_child() {
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": "qwen3.8-max-preview",
        "prompt": "research"
    });

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "qwen3.8-max-preview");
}

#[test]
fn explicit_cline_worker_from_codex_luna_parent_stays_cline() {
    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-cline-deepseek-flash",
        "claudex_model": "cline-pass/deepseek-v4-flash",
        "claudex_effort": "xhigh",
        "prompt": "explicit cline launch"
    });

    hydrate_standard_agent_to_parent(&mut arguments, CODEX_LUNA, &gpt_luna_catalog());

    assert_eq!(arguments[ADAPTER_MODEL], "cline-pass/deepseek-v4-flash");
}
