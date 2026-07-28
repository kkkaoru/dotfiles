use anyhow::{Result, bail};

use super::MessagesRequest;

/// Apply SubAgent intent overrides and policy denylist / unrouted-provider remaps.
pub(super) fn resolve_request_model(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
    intent_matched: bool,
    model_override: Option<String>,
    supports_model: impl Fn(&str) -> bool,
    // True when the model matches any provider identity declared in config (enabled or not).
    is_declared_provider_model: impl Fn(&str) -> bool,
) -> Result<RouteDecision> {
    if is_subagent && !intent_matched {
        bail!("SubAgent request did not match an explicit Agent/Task launch intent");
    }
    if is_subagent && model_override.is_none() {
        bail!("SubAgent launch is missing required explicit `claudex_model`");
    }
    if let Some(model) = model_override {
        request.model = model;
    }
    if let Some(model) = request
        .model
        .strip_prefix(crate::DISCOVERY_MODEL_PREFIX)
        .filter(|model| *model == main_model || supports_model(model))
        .map(str::to_owned)
    {
        request.model = model;
    }

    apply_disabled_model_policy(request, main_model, is_subagent)?;

    if request.model.is_empty() || request.model == main_model || supports_model(&request.model) {
        return Ok(RouteDecision::Provider);
    }
    if is_declared_provider_model(&request.model) {
        if is_subagent {
            bail!(
                "SubAgent model `{}` is declared but has no active provider route",
                request.model
            );
        }
        tracing::warn!(
            request_model = %request.model,
            main_model,
            "remapping request for an unrouted provider model onto the adapter main route"
        );
        request.model = main_model.to_owned();
        return Ok(RouteDecision::Provider);
    }
    Ok(RouteDecision::Subscription)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RouteDecision {
    Provider,
    Subscription,
}

fn apply_disabled_model_policy(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
) -> Result<()> {
    if !request.disabled_subagent_models.contains(&request.model) {
        return Ok(());
    }
    if is_subagent {
        bail!(
            "SubAgent model `{}` is disabled by the active Claudex policy ({}/disabledModels)",
            request.model,
            crate::subagent_policy::ENV_NAME
        );
    }
    if request.model == main_model {
        // The dedicated policy controls only spawned workers. A terminal may
        // keep using a model in its outer session while denying that same
        // model to SubAgents.
        return Ok(());
    }
    tracing::warn!(
        disabled_model = %request.model,
        main_model,
        "remapping outer request off a policy-disabled model onto the adapter main route"
    );
    request.model = main_model.to_owned();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn rejects_an_unrouted_declared_subagent_model_instead_of_using_main() {
        let mut request = request("vendor-offline-1", &[]);
        let error = resolve(
            &mut request,
            "main-model",
            true,
            |_| false,
            |model| model.starts_with("vendor-"),
        )
        .expect_err("unrouted declared SubAgent model");
        assert!(error.to_string().contains("has no active provider route"));
        assert_eq!(request.model, "vendor-offline-1");
    }

    #[test]
    fn still_rejects_disabled_subagent_models() {
        let mut request = request("vendor-a", &["vendor-a"]);
        let error = resolve(
            &mut request,
            "main-model",
            true,
            |model| model == "main-model",
            |model| model == "vendor-a" || model == "main-model",
        )
        .expect_err("deny");
        assert!(
            error
                .to_string()
                .contains("disabled by the active Claudex policy")
        );
    }

    #[test]
    fn rejects_unmatched_or_model_less_subagent_requests() {
        let mut unmatched = request("claude-sonnet-5", &[]);
        let error = resolve_request_model(
            &mut unmatched,
            "main-model",
            true,
            false,
            None,
            |_| false,
            |_| false,
        )
        .expect_err("unmatched SubAgent");
        assert!(error.to_string().contains("did not match"));
        assert_eq!(unmatched.model, "claude-sonnet-5");

        let mut missing = request("main-model", &[]);
        let error = resolve_request_model(
            &mut missing,
            "main-model",
            true,
            true,
            None,
            |_| true,
            |_| true,
        )
        .expect_err("model-less SubAgent");
        assert!(
            error
                .to_string()
                .contains("missing required explicit `claudex_model`")
        );
    }
}
