use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::Bridge;
use crate::provider_config;

#[test]
fn routed_models_advertise_codex_terra_for_main_selection() {
    let catalog = provider_config::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.config/claudex/providers.json")
            .as_path(),
    )
    .expect("load repository providers.json")
    .model_catalog;
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "gpt-5.6-luna",
            BackendKind::CodexAppServer,
        )]),
        "gpt-5.6-luna".to_owned(),
    )
    .with_model_catalog(catalog);
    let models = bridge.routed_models();
    assert!(
        models.iter().any(|model| model == "gpt-5.6-luna"),
        "default Codex model must stay listed: {models:?}"
    );
    assert!(
        models.iter().any(|model| model == "gpt-5.6-terra"),
        "gpt-5.6-terra must appear on GET /v1/models: {models:?}"
    );
}
