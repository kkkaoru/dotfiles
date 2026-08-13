use crate::provider_config::ModelCatalog;

const OPENCODE_GO_LUNA_AGENT: &str = "claudex-opencode-gpt";
const CODEX_GPT_AGENT: &str = "claudex-gpt";
const GPT_LUNA_AGENTS: &[&str] = &[OPENCODE_GO_LUNA_AGENT, CODEX_GPT_AGENT];

pub(crate) fn is_cline_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("cline/") || model.starts_with("cline-pass/")
}

pub(crate) fn is_gpt_luna_model(model: &str, catalog: &ModelCatalog) -> bool {
    let model = model.trim();
    GPT_LUNA_AGENTS.iter().any(|agent| {
        catalog
            .worker_fields(agent)
            .is_some_and(|(configured, _)| configured == model)
    })
}

pub(crate) fn opencode_go_luna_model(catalog: &ModelCatalog) -> Option<&str> {
    catalog
        .worker_fields(OPENCODE_GO_LUNA_AGENT)
        .map(|(model, _)| model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_config::{ModelCatalog, WorkerRoute};

    fn catalog_with(workers: &[(&str, &str)]) -> ModelCatalog {
        let mut catalog = ModelCatalog::default();
        catalog
            .set_worker_routes(
                workers
                    .iter()
                    .map(|(agent, model)| WorkerRoute::new(*agent, *model, "max"))
                    .collect(),
            )
            .expect("install workers");
        catalog
    }

    fn luna_catalog() -> ModelCatalog {
        catalog_with(&[
            (CODEX_GPT_AGENT, "gpt-5.6-luna"),
            (OPENCODE_GO_LUNA_AGENT, "opencode-go/gpt-5.6-luna"),
        ])
    }

    #[test]
    fn detects_cline_credits_and_clinepass_models() {
        assert!(is_cline_model("cline-pass/deepseek-v4-flash"));
        assert!(is_cline_model("cline/deepseek-v4-flash"));
        assert!(!is_cline_model("gpt-5.6-luna"));
        assert!(!is_cline_model("opencode-go/gpt-5.6-luna"));
        assert!(!is_cline_model("qwen3.8-max-preview"));
    }

    #[test]
    fn detects_codex_and_opencode_gpt_luna_from_catalog() {
        let catalog = luna_catalog();
        assert!(is_gpt_luna_model("gpt-5.6-luna", &catalog));
        assert!(is_gpt_luna_model("opencode-go/gpt-5.6-luna", &catalog));
        assert!(!is_gpt_luna_model("gpt-5.3-codex-spark", &catalog));
        assert!(!is_gpt_luna_model("cline-pass/deepseek-v4-flash", &catalog));
        assert_eq!(
            opencode_go_luna_model(&catalog),
            Some("opencode-go/gpt-5.6-luna")
        );
    }

    #[test]
    fn luna_identity_follows_catalog_workers_not_model_string_shape() {
        let catalog = catalog_with(&[
            (CODEX_GPT_AGENT, "codex-luna-from-catalog"),
            (OPENCODE_GO_LUNA_AGENT, "opencode-go/catalog-luna"),
        ]);
        assert!(is_gpt_luna_model("codex-luna-from-catalog", &catalog));
        assert!(is_gpt_luna_model("opencode-go/catalog-luna", &catalog));
        assert!(!is_gpt_luna_model("gpt-5.6-luna", &catalog));
        assert!(!is_gpt_luna_model("opencode-go/gpt-5.6-luna", &catalog));
        assert!(!is_gpt_luna_model("gpt-5.6-luna", &ModelCatalog::default()));
        assert_eq!(opencode_go_luna_model(&ModelCatalog::default()), None);
        assert_eq!(
            opencode_go_luna_model(&catalog),
            Some("opencode-go/catalog-luna")
        );
    }
}
