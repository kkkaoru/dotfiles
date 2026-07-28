use std::{fs, path::Path};

#[test]
fn codex_fish_routes_external_models_to_their_provider_profiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let launcher = fs::read_to_string(root.join(".config/fish/functions/codex.fish"))
        .expect("Codex fish launcher");
    for expected in [
        "case fugu 'fugu-*'",
        "set -p codex_argv --profile fugu",
        "case 'glm-5.2:cloud'",
        "set -p codex_argv --profile ollama-launch-codex-app",
    ] {
        assert!(
            launcher.contains(expected),
            "missing launcher route: {expected}"
        );
    }

    let catalog: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".codex/fugu.json")).expect("shared Codex model catalog"),
    )
    .expect("valid shared Codex model catalog");
    assert!(
        catalog["models"]
            .as_array()
            .is_some_and(|models| { models.iter().any(|model| model["slug"] == "glm-5.2:cloud") })
    );

    for (profile, provider, base_url) in [
        ("fugu.config.toml", "sakana", "https://api.sakana.ai/v1"),
        (
            "ollama-launch-codex-app.config.toml",
            "ollama-launch-codex-app",
            "http://127.0.0.1:11434/v1",
        ),
    ] {
        let config =
            fs::read_to_string(root.join(".codex").join(profile)).expect("Codex provider profile");
        assert!(
            config.contains(&format!("model_provider = \"{provider}\"")),
            "{profile} must select {provider}"
        );
        assert!(config.contains(&format!("[model_providers.{provider}]")));
        assert!(config.contains(&format!("base_url = \"{base_url}\"")));
    }
}

#[test]
fn codex_model_catalog_includes_parallel_execution_guidance_for_gpt_56_sol() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let catalog: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".codex/fugu.json")).expect("shared Codex model catalog"),
    )
    .expect("valid shared Codex model catalog");

    let phrase = "In Code tasks, avoid serializing independent operations";
    let models = catalog["models"]
        .as_array()
        .expect("models exists in Codex catalog");

    for model in models {
        let slug = model["slug"]
            .as_str()
            .unwrap_or("<unknown>");
        assert!(
            model["base_instructions"]
                .as_str()
                .is_some_and(|instructions| instructions.contains(phrase)),
            "{slug} base instructions should include parallel execution guidance"
        );

        if let Some(model_messages) = model["model_messages"].as_object() {
            if let Some(template) = model_messages.get("instructions_template").and_then(|v| v.as_str()) {
                assert!(
                    template.contains(phrase),
                    "{slug} instructions template should include parallel execution guidance"
                );
            }
        }
    }
}
