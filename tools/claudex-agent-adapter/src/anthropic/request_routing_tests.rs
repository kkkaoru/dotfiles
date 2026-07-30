#[cfg(test)]
// Coverage gates measure production routing; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Once;

    use super::*;
    use serde_json::json;

    fn enable_warning_logs() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_test_writer()
                .try_init();
        });
    }

    fn request(model: &str, disabled: &[&str]) -> MessagesRequest {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model": model,
            "messages": [{"role":"user","content":"hi"}]
        }))
        .expect("request");
        request.disabled_subagent_models = disabled.iter().map(|item| (*item).to_owned()).collect();
        request
    }

    fn resolve(
        request: &mut MessagesRequest,
        main: &str,
        is_subagent: bool,
        supports: impl Fn(&str) -> bool,
        declared: impl Fn(&str) -> bool,
    ) -> Result<RouteDecision> {
        let model_override = is_subagent.then(|| request.model.clone());
        resolve_request_model(
            request,
            main,
            is_subagent,
            true,
            model_override,
            supports,
            declared,
        )
    }

    #[test]
    fn remaps_disabled_outer_provider_model_to_main() {
        enable_warning_logs();
        let mut request = request("vendor-a", &["vendor-a"]);
        let decision = resolve(
            &mut request,
            "main-model",
            false,
            |model| model == "main-model",
            |model| model == "vendor-a" || model == "main-model",
        )
        .expect("remap");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "main-model");
    }

    #[test]
    fn keeps_a_disabled_model_for_the_outer_main_session() {
        let mut request = request("main-model", &["main-model"]);
        let decision = resolve(
            &mut request,
            "main-model",
            false,
            |model| model == "main-model",
            |model| model == "main-model",
        )
        .expect("outer main model remains allowed");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "main-model");
    }

    #[test]
    fn remaps_unrouted_declared_provider_model_to_main_instead_of_subscription() {
        enable_warning_logs();
        let mut request = request("vendor-offline-1", &[]);
        let decision = resolve(
            &mut request,
            "main-model",
            false,
            |_| false,
            |model| model.starts_with("vendor-"),
        )
        .expect("remap");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "main-model");
    }

    #[test]
    fn keeps_subscription_for_undeclared_models() {
        let mut request = request("claude-sonnet-5", &[]);
        let decision = resolve(
            &mut request,
            "main-model",
            false,
            |_| false,
            |model| model.starts_with("vendor-"),
        )
        .expect("subscription");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(request.model, "claude-sonnet-5");
    }

    #[test]
    fn keeps_an_unsupported_discovery_alias_on_subscription() {
        let mut request = request("claude-claudex-unknown", &[]);
        let decision = resolve_request_model(
            &mut request,
            "main-model",
            false,
            true,
            None,
            |_| false,
            |_| false,
        )
        .expect("unsupported discovery alias should remain a subscription");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(request.model, "claude-claudex-unknown");
    }

    #[test]
    fn normalizes_supported_discovery_aliases_before_routing() {
        let mut main = request("claude-claudex-main-model", &[]);
        let decision = resolve_request_model(
            &mut main,
            "main-model",
            false,
            true,
            None,
            |_| false,
            |_| false,
        )
        .expect("main discovery alias");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(main.model, "main-model");

        let mut supported = request("claude-claudex-vendor-model", &[]);
        let decision = resolve_request_model(
            &mut supported,
            "main-model",
            false,
            true,
            None,
            |model| model == "vendor-model",
            |_| false,
        )
        .expect("provider discovery alias");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(supported.model, "vendor-model");
    }

    #[test]
    fn routes_empty_main_and_supported_models_to_the_provider() {
        for model in ["", "main-model", "vendor-model"] {
            let mut request = request(model, &[]);
            let decision = resolve_request_model(
                &mut request,
                "main-model",
                false,
                true,
                None,
                |candidate| candidate == "vendor-model",
                |_| false,
            )
            .expect("provider route");
            assert_eq!(decision, RouteDecision::Provider, "model={model}");
        }
    }

    #[test]
    fn rejects_an_unrouted_declared_subagent_model_without_fallback() {
        enable_warning_logs();
        let mut request = request("vendor-offline-1", &[]);
        let decision = resolve(
            &mut request,
            "main-model",
            true,
            |_| false,
            |model| model.starts_with("vendor-"),
        )
        .expect_err("unrouted SubAgent model must not be remapped");
        assert!(
            decision
                .to_string()
                .contains("does not have a recoverable configured route")
        );
    }

    #[test]
    fn rejects_disabled_subagent_models_without_fallback() {
        enable_warning_logs();
        let mut request = request("vendor-a", &["vendor-a"]);
        let decision = resolve(
            &mut request,
            "main-model",
            true,
            |model| model == "main-model",
            |model| model == "vendor-a" || model == "main-model",
        )
        .expect_err("disabled SubAgent model must not be routed");
        assert!(
            decision
                .to_string()
                .contains("disabled by the active Claudex policy")
        );
    }

    #[test]
    fn routes_correlated_parent_and_provider_subagents() {
        let mut unmatched = request("claude-sonnet-5", &[]);
        let decision = resolve_request_model(
            &mut unmatched,
            "main-model",
            true,
            false,
            None,
            |_| false,
            |_| false,
        )
        .expect("unmatched native child must use the subscription backend");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(unmatched.model, super::models::CLAUDE_HAIKU_MODEL);

        let mut inherited = request("claude-sonnet-5", &[]);
        let decision = resolve_request_model(
            &mut inherited,
            "main-model",
            true,
            false,
            Some("gpt-parent-model".to_owned()),
            |model| model == "gpt-parent-model",
            |_| false,
        )
        .expect("a correlated parent model must win over the Claude request model");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(inherited.model, "gpt-parent-model");

        let mut provider_child = request("main-model", &[]);
        let decision = resolve_request_model(
            &mut provider_child,
            "main-model",
            true,
            false,
            None,
            |model| model == "main-model",
            |_| false,
        )
        .expect("unmatched provider child must use its active route");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(provider_child.model, "main-model");

        let mut missing = request("main-model", &[]);
        let decision = resolve_request_model(
            &mut missing,
            "main-model",
            true,
            true,
            None,
            |_| true,
            |_| true,
        )
        .expect("model-less provider SubAgent must use its active route");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(missing.model, "main-model");
    }

    #[test]
    fn normalizes_native_subagent_models_from_origin() {
        let mut missing_native = request("claude-sonnet-5", &[]);
        let decision = resolve_request_model(
            &mut missing_native,
            "main-model",
            true,
            true,
            None,
            |_| false,
            |_| false,
        )
        .expect("model-less native SubAgent must use the subscription backend");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(
            missing_native.model,
            super::models::CLAUDE_HAIKU_MODEL
        );

        let mut inherited_native = request("claude-sonnet-5", &[]);
        let decision = resolve_request_model_with_origin(
            &mut inherited_native,
            "main-model",
            Some("claude-sonnet-5".to_owned()),
            RouteOrigin::new(true, true, true),
            |_| false,
            |_| false,
        )
        .expect("an inherited Claude parent must use the Haiku route");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(inherited_native.model, super::models::CLAUDE_HAIKU_MODEL);

        let mut explicit_native = request("claude-sonnet-5", &[]);
        let decision = resolve_request_model_with_origin(
            &mut explicit_native,
            "main-model",
            Some("claude-opus-5".to_owned()),
            RouteOrigin::new(true, true, false),
            |_| false,
            |_| false,
        )
        .expect("an explicitly selected Claude child model must be preserved");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(explicit_native.model, "claude-opus-5");
    }

    #[test]
    fn rejects_unknown_subagent_model() {
        let mut empty = request("", &[]);
        let decision = resolve_request_model(
            &mut empty,
            "main-model",
            true,
            false,
            None,
            |_| false,
            |_| false,
        )
        .expect_err("an unknown SubAgent model must not inherit the main route");
        assert!(
            decision
                .to_string()
                .contains("does not have a recoverable configured route")
        );
    }
}
