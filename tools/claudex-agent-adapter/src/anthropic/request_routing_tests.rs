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
    fn keeps_disabled_outer_provider_model_authoritative() {
        enable_warning_logs();
        let mut request = request("vendor-a", &["vendor-a"]);
        let decision = resolve(
            &mut request,
            "main-model",
            false,
            |model| model == "vendor-a",
            |model| model == "vendor-a" || model == "main-model",
        )
        .expect("outer request model remains authoritative");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "vendor-a");
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
    fn rejects_unrouted_declared_provider_model_without_remapping() {
        enable_warning_logs();
        let mut request = request("vendor-offline-1", &[]);
        let error = resolve(
            &mut request,
            "main-model",
            false,
            |model| model == "main-model",
            |model| model.starts_with("vendor-"),
        )
        .expect_err("unavailable provider model must fail explicitly");
        assert!(error.to_string().contains("does not have an active route"));
        assert_eq!(request.model, "vendor-offline-1");
    }

    #[test]
    fn keeps_subscription_for_each_undeclared_outer_model() {
        for model in ["claude-opus-5", "claude-sonnet-5", "claude-fable"] {
            let mut request = request(model, &[model]);
            let decision = resolve(
                &mut request,
                "main-model",
                false,
                |_| false,
                |candidate| candidate.starts_with("vendor-"),
            )
            .expect("subscription");
            assert_eq!(decision, RouteDecision::Subscription);
            assert_eq!(request.model, model);
        }
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
            |model| model == "main-model",
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
            |model| model == "main-model",
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
    fn routes_each_supported_outer_model_to_its_provider() {
        for model in ["main-model", "vendor-model"] {
            let mut request = request(model, &[]);
            let decision = resolve_request_model(
                &mut request,
                "main-model",
                false,
                true,
                None,
                |candidate| candidate == model,
                |_| false,
            )
            .expect("provider route");
            assert_eq!(decision, RouteDecision::Provider, "model={model}");
            assert_eq!(request.model, model);
        }
    }

    #[test]
    fn rejects_a_missing_outer_model_instead_of_selecting_a_provider() {
        let mut request = request("", &[]);
        let error = resolve_request_model(
            &mut request,
            "main-model",
            false,
            true,
            None,
            |_| true,
            |_| true,
        )
        .expect_err("missing model must fail");
        assert!(error.to_string().contains("request model is required"));
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
    fn keeps_an_explicit_same_model_subagent_on_the_provider_route() {
        let mut request = request("main-model", &[]);
        let decision = resolve_request_model_with_origin(
            &mut request,
            "main-model",
            Some("main-model".to_owned()),
            RouteOrigin::new(true, true, false),
            |model| model == "main-model",
            |_| false,
        )
        .expect("an explicit same-model worker should use the provider route");

        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "main-model");
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
        assert_eq!(missing_native.model, super::models::CLAUDE_HAIKU_MODEL);

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
    fn session_id_header_keeps_oversized_outer_claude_model() {
        let mut request = request("claude-fable-5", &[]);
        request.system = json!(
            "cc_is_subagent=true\n<claudex-agent-id>archived-system-agent</claudex-agent-id>"
        );
        request.messages = vec![json!({
            "role": "user",
            "content": "x".repeat(400_000)
        })];
        crate::anthropic::RequestIdentity::new(
            Some("outer-model-authority".to_owned()),
            None,
            None,
        )
        .attach(&mut request);

        let decision = resolve_request_model(
            &mut request,
            "gpt-5.6-luna",
            true,
            false,
            None,
            |_| false,
            |_| false,
        )
        .expect("session_id-only outer continue must keep the requested Claude model");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(request.model, "claude-fable-5");
    }

    #[test]
    fn session_id_header_keeps_a_provider_model_disabled_only_for_subagents() {
        let mut request = request("gpt-5.6-luna", &["gpt-5.6-luna"]);
        request.system =
            json!("cc_is_subagent=true\n<claudex-agent-id>archived-child</claudex-agent-id>");
        crate::anthropic::RequestIdentity::new(Some("main-session-only".to_owned()), None, None)
            .attach(&mut request);

        let decision = resolve_request_model(
            &mut request,
            "gpt-5.6-luna",
            true,
            false,
            None,
            |model| model == "gpt-5.6-luna",
            |_| false,
        )
        .expect("outer main may use a model that is disabled only for SubAgents");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(request.model, "gpt-5.6-luna");
    }

    #[test]
    fn agent_id_header_still_rewrites_an_oversized_native_child() {
        let mut request = request("claude-fable-5", &[]);
        request.messages = vec![json!({
            "role": "user",
            "content": "x".repeat(400_000)
        })];
        crate::anthropic::RequestIdentity::new(
            Some("main-session".to_owned()),
            Some("agent-child".to_owned()),
            None,
        )
        .attach(&mut request);

        let decision = resolve_request_model(
            &mut request,
            "gpt-5.6-luna",
            true,
            false,
            None,
            |_| false,
            |_| false,
        )
        .expect("a live child still uses the long-context subscription model");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(request.model, super::models::CLAUDE_LONG_CONTEXT_MODEL);
    }

    #[test]
    fn routes_an_oversized_native_subagent_to_the_long_context_model() {
        let mut oversized = request("claude-sonnet-5", &[]);
        oversized.messages = vec![json!({
            "role": "user",
            "content": "x".repeat(400_000)
        })];
        let decision = resolve_request_model(
            &mut oversized,
            "main-model",
            true,
            true,
            None,
            |_| false,
            |_| false,
        )
        .expect("oversized native SubAgent must remain on a subscription route");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(oversized.model, super::models::CLAUDE_LONG_CONTEXT_MODEL);
    }

    #[test]
    fn preserves_an_inherited_long_context_subscription_model() {
        let mut long_context_request = request(super::models::CLAUDE_LONG_CONTEXT_MODEL, &[]);
        let decision = resolve_request_model_with_origin(
            &mut long_context_request,
            "main-model",
            None,
            RouteOrigin::new(true, true, true),
            |_| false,
            |_| false,
        )
        .expect("inherited long-context model");
        assert_eq!(decision, RouteDecision::Subscription);
        assert_eq!(
            long_context_request.model,
            super::models::CLAUDE_LONG_CONTEXT_MODEL
        );

        let mut inherited_worker = request("worker-model", &[]);
        let error = resolve_request_model_with_origin(
            &mut inherited_worker,
            "main-model",
            Some("worker-model".to_owned()),
            RouteOrigin::new(true, true, true),
            |_| false,
            |_| false,
        )
        .expect_err("an inherited unsupported worker must not be remapped");
        assert!(error.to_string().contains("recoverable configured route"));
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
        assert!(decision.to_string().contains("request model is required"));
    }

    #[test]
    fn direct_subagent_with_explicit_model_debugs_instead_of_warns() {
        enable_warning_logs();
        let mut req = request("gpt-5.6-luna", &[]);
        // Direct SubAgent request with explicit model (no prior Agent/Task intent).
        // This should route normally without a WARN-level noise.
        let decision = resolve_request_model_with_origin(
            &mut req,
            "main-model",
            None,
            RouteOrigin::new(true, false, false),
            |model| model == "gpt-5.6-luna",
            |_| false,
        )
        .expect("direct SubAgent with valid model");
        assert_eq!(decision, RouteDecision::Provider);
        assert_eq!(req.model, "gpt-5.6-luna");
    }
}
