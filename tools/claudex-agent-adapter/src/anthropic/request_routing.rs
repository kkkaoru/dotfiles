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
    if let Some(model) = model_override {
        request.model = model;
    } else if is_subagent && !intent_matched && !supports_model(&request.model) {
        // Unmatched SubAgents may carry Claude Code's subscription fallback model.
        request.model = main_model.to_owned();
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
        resolve_request_model(request, main, is_subagent, true, None, supports, declared)
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
        assert!(error.to_string().contains("disabled by the active Claudex policy"));
    }
}
