use serde_json::Value;

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::request_routing::RouteDecision;
use crate::anthropic::{Bridge, MessagesRequest};
use crate::provider_config::ModelCatalog;

use super::super::segment::EMPTY_ACP_END_TURN;
use super::{UsageLimitFailover, should_failover_provider_error, streaming_provider_retry};

const CLINE_FLASH: &str = "cline-pass/deepseek-v4-flash";
const QWEN_CLOUD: &str = "qwen3.8-max-preview";
const CURSOR_AUTO: &str = "auto";
const SPARK: &str = "gpt-5.3-codex-spark";
const LUNA: &str = "gpt-5.6-luna";

/// Exact Claude Code / TUI wording from session `fa522331-…`
/// (`Verify r2-catalog inherited edits` / `Verify weight-triggered re-prediction`).
const TUI_EMPTY_ACP: &str = "Agent terminated early due to an API error: API Error: \
Configured ACP completed with no assistant content (provider likely unavailable or billing exhausted; \
Cline Credits models return empty end_turn when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / \
`claudex-qwen`, or top up Credits)";
const TUI_EMPTY_ACP_502: &str = "Agent terminated early due to an API error: API Error: 502 \
Configured ACP completed with no assistant content (provider likely unavailable or billing exhausted; \
Cline Credits models return empty end_turn when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / \
`claudex-qwen`, or top up Credits). This is a server-side issue, usually temporary — try again in a moment. \
If it persists, check your inference gateway (127.0.0.1:54304).";

fn cline_and_qwen_bridge() -> Bridge {
    let cache_home = Box::leak(Box::new(
        tempfile::tempdir().expect("isolated failover cache"),
    ));
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(CLINE_FLASH, BackendKind::ConfiguredAcp),
        BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new(
                "claudex-cline-deepseek-flash",
                CLINE_FLASH,
                "xhigh",
            ),
            crate::provider_config::WorkerRoute::new("claudex-qwen", QWEN_CLOUD, "high"),
        ])
        .expect("install workers");
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    Bridge::new_with_backend(backend, CLINE_FLASH.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(cache_home.path())
}

#[test]
fn prefers_configured_subscription_fallback_before_other_providers() {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new("fugu", BackendKind::CodexAppServer),
        BackendRoute::new("grok-4.5", BackendKind::GrokAcp),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install fallback");
    let bridge = Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
    let failover = bridge
        .usage_limit_failover_for("fugu")
        .expect("failover target");
    assert_eq!(failover.model, "claude-sonnet-5");
    assert_eq!(failover.route, RouteDecision::Subscription);
}

#[test]
fn falls_back_to_configured_subscription_when_only_codex_remains() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new("fugu", BackendKind::CodexAppServer)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install fallback");
    let bridge = Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
    let failover = bridge
        .usage_limit_failover_for("fugu")
        .expect("subscription failover");
    assert_eq!(failover.model, "claude-sonnet-5");
    assert_eq!(failover.route, RouteDecision::Subscription);
}

#[test]
fn treats_sakana_invalid_api_key_as_failover_trigger() {
    assert!(super::should_failover_provider_error(&anyhow::anyhow!(
        "codex app-server turn failed: unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
    )));
}

#[test]
fn subscription_oauth_expiry_does_not_trigger_provider_to_subscription_failover() {
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Please run /login · API Error: 401 Claude subscription model claude-opus-5 failed \
[authentication; exit status: 1; trace=2d31ef8d8e4d4288ae0b68c99c2eb18b]: \
Failed to authenticate: OAuth session expired and could not be refreshed"
    )));
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Please run /login · API Error: 401 Claude subscription model claude-opus-5 failed \
[authentication; exit status: 1; trace=7210210d09a547d8909f1f419d4b9d04]: \
Failed to authenticate: OAuth session expired and could not be refreshed"
    )));
}

#[test]
fn treats_429_rate_limit_as_failover_trigger() {
    assert!(super::should_failover_provider_error(&anyhow::anyhow!(
        "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests, request id: abc"
    )));
    assert!(super::should_failover_provider_error(&anyhow::Error::msg(
        r#"codex app-server turn failed: {"error":{"codexErrorInfo":{"responseTooManyFailedAttempts":{"httpStatusCode":429}},"message":"exceeded retry limit"}}"#
    )));
}

#[test]
fn treats_empty_acp_billing_as_failover_trigger() {
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        EMPTY_ACP_END_TURN
    )));
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        TUI_EMPTY_ACP
    )));
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        TUI_EMPTY_ACP_502
    )));
}

#[test]
fn prefers_qwen_cloud_sibling_for_cline_empty_acp() {
    let failover = cline_and_qwen_bridge()
        .subagent_provider_failover_for(CLINE_FLASH)
        .expect("sibling provider");
    assert_eq!(failover.model, QWEN_CLOUD);
    assert_eq!(failover.route, RouteDecision::Provider);
    assert_eq!(failover.effort.as_deref(), Some("high"));
}

#[test]
fn regression_subagent_empty_acp_no_longer_dies_on_subscription_stream() {
    // Historical bug (fa522331 horse-racing TUI):
    // empty Cline ACP was classified as usage-limit and failover_for() returned
    // Subscription. streaming_provider_retry(Subscription) is None, so the
    // already-open SubAgent SSE turn emitted EMPTY_ACP_END_TURN to Claude Code
    // ("Agent terminated early due to an API error").
    let bridge = cline_and_qwen_bridge();
    let outer_style = bridge.usage_limit_failover_for(CLINE_FLASH);
    assert_eq!(
        outer_style.as_ref().map(|failover| failover.route),
        Some(RouteDecision::Subscription)
    );
    assert!(
        streaming_provider_retry(outer_style).is_none(),
        "subscription cannot continue an already-open SubAgent stream"
    );

    let retry = streaming_provider_retry(bridge.failover_for_stream_turn(CLINE_FLASH, true))
        .expect("SubAgent empty ACP must retry a sibling Provider");
    assert_eq!(retry.model, QWEN_CLOUD);
    assert_eq!(retry.route, RouteDecision::Provider);
}

#[test]
fn outer_stream_empty_acp_still_uses_subscription_preflight_not_inline_retry() {
    let bridge = cline_and_qwen_bridge();
    let failover = bridge.failover_for_stream_turn(CLINE_FLASH, false);
    assert_eq!(
        failover.as_ref().map(|candidate| candidate.route),
        Some(RouteDecision::Subscription)
    );
    assert!(streaming_provider_retry(failover).is_none());
}

#[test]
fn empty_acp_records_cline_cooldown_without_codex_usage_limit() {
    use std::time::SystemTime;

    use crate::anthropic::{provider_auth_cooldown, usage_limit_cooldown};

    let root = tempfile::tempdir().expect("empty-acp cooldown fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(CLINE_FLASH));
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    assert!(bridge.subagent_provider_is_exhausted(CLINE_FLASH));
    assert!(provider_auth_cooldown::scope_is_cooling_down_at(
        bridge.provider_auth_cache_path().as_deref(),
        CLINE_FLASH,
        SystemTime::now(),
    ));
    assert!(!bridge.subagent_provider_is_exhausted(QWEN_CLOUD));
    assert!(!usage_limit_cooldown::codex_app_server_is_cooling_down_at(
        bridge.usage_limit_cache_path().as_deref(),
        SystemTime::now(),
    ));
    assert!(
        streaming_provider_retry(bridge.failover_for_stream_turn(CLINE_FLASH, true))
            .is_some_and(|retry| retry.model == QWEN_CLOUD),
        "after Cline cooldown, SubAgent stream must still land on Qwen Cloud"
    );
}

#[test]
fn records_429_cooldown_per_model_without_backend_usage_limit() {
    use std::time::SystemTime;

    use crate::anthropic::{provider_auth_cooldown, usage_limit_cooldown};

    let root = tempfile::tempdir().expect("rate-limit cooldown fixture");
    let mut route = BackendRoute::new("glm-5.2:cloud", BackendKind::CodexAppServer);
    route.model_provider = Some("ollama".to_owned());
    let backend = AgentBackend::spawn_routes(&[route]);
    let bridge = Bridge::new_with_backend(backend, "glm-5.2:cloud".to_owned())
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!(
            "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests"
        ),
        Some("glm-5.2:cloud"),
    );
    assert!(bridge.subagent_provider_is_exhausted("glm-5.2:cloud"));
    assert!(provider_auth_cooldown::scope_is_cooling_down_at(
        bridge.provider_auth_cache_path().as_deref(),
        "glm-5.2:cloud",
        SystemTime::now(),
    ));
    assert!(provider_auth_cooldown::scope_is_cooling_down_at(
        bridge.provider_auth_cache_path().as_deref(),
        "ollama",
        SystemTime::now(),
    ));
    assert!(!usage_limit_cooldown::codex_app_server_is_cooling_down_at(
        bridge.usage_limit_cache_path().as_deref(),
        SystemTime::now(),
    ));
}

#[test]
fn rewrites_exhausted_cline_nested_launch_onto_named_qwen_worker() {
    let root = tempfile::tempdir().expect("rewrite fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": CLINE_FLASH,
        "claudex_effort": "xhigh",
        "prompt": "continue after empty ACP"
    });

    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);

    assert_eq!(arguments["subagent_type"], "claudex-qwen");
    assert_eq!(arguments["claudex_model"], QWEN_CLOUD);
    assert_eq!(arguments["claudex_effort"], "high");
    assert!(
        !bridge.subagent_provider_is_exhausted(QWEN_CLOUD),
        "Qwen sibling must remain launchable after Cline cooldown"
    );
}

fn cline_qwen_cursor_bridge() -> Bridge {
    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp);
    qwen.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(CLINE_FLASH, BackendKind::ConfiguredAcp),
        qwen,
        BackendRoute::new(CURSOR_AUTO, BackendKind::ConfiguredAcp),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new(
                "claudex-cline-deepseek-flash",
                CLINE_FLASH,
                "xhigh",
            ),
            crate::provider_config::WorkerRoute::new("claudex-qwen", QWEN_CLOUD, "high"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", CURSOR_AUTO, "high"),
        ])
        .expect("install workers");
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog)
}

async fn saturate_qwen_subagent_slots(
    bridge: &Bridge,
) -> Vec<crate::anthropic::model_concurrency::ModelPermit> {
    let mut permits = Vec::new();
    for _ in 0..2 {
        permits.push(
            bridge
                .model_concurrency
                .ticket(QWEN_CLOUD, Some(3))
                .expect("qwen ticket")
                .acquire_for(false)
                .await
                .expect("qwen subagent slot"),
        );
    }
    assert!(bridge.model_concurrency.is_subagent_at_capacity(QWEN_CLOUD));
    permits
}

fn dummy_request(model: &str) -> MessagesRequest {
    MessagesRequest {
        model: model.to_owned(),
        system: Value::Null,
        messages: Vec::new(),
        tools: Vec::new(),
        stream: true,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

#[test]
fn rewrites_exhausted_cline_http_subagent_request_onto_qwen() {
    // Historical bug: outer Subscription launched claudex-cline-deepseek-flash
    // after Cline empty-ACP cooldown. message_router hard-rejected with
    // `502 provider for model 'cline-pass/deepseek-v4-flash' is cooling down`.
    let root = tempfile::tempdir().expect("http rewrite fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("sibling rewrite");
    assert_eq!(request.model, QWEN_CLOUD);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn exhausted_ollama_glm_subagent_http_is_rejected() {
    let root = tempfile::tempdir().expect("ollama glm fixture");
    let mut route = BackendRoute::new("glm-5.2:cloud", BackendKind::CodexAppServer);
    route.model_provider = Some("ollama".to_owned());
    let backend = AgentBackend::spawn_routes(&[
        route,
        BackendRoute::new("gpt-5.6-luna", BackendKind::CodexAppServer),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new(
                "claudex-ollama-glm-5-2",
                "glm-5.2:cloud",
                "max",
            )
            .with_usage_provider(Some("ollama".to_owned())),
            crate::provider_config::WorkerRoute::new("claudex-gpt", "gpt-5.6-luna", "max"),
        ])
        .expect("install glm worker");
    let bridge = Bridge::new_with_backend(backend, "glm-5.2:cloud".to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!(
            "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests"
        ),
        Some("glm-5.2:cloud"),
    );
    assert!(bridge.subagent_provider_is_exhausted("glm-5.2:cloud"));
    let mut request = dummy_request("glm-5.2:cloud");
    let mut effort = Some("max".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("ollama glm must not start a SubAgent turn while cooling down");
    assert!(
        error
            .to_string()
            .contains("cooling down after rate/usage/billing limit"),
        "{error}"
    );
    assert_eq!(request.model, "glm-5.2:cloud");
}

#[test]
fn exhausted_subagent_without_sibling_still_rejects() {
    let root = tempfile::tempdir().expect("no sibling fixture");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::ConfiguredAcp)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-cline-deepseek-flash",
            CLINE_FLASH,
            "xhigh",
        )])
        .expect("install cline worker");
    let bridge = Bridge::new_with_backend(backend, CLINE_FLASH.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("no sibling remains a cooldown reject");
    assert!(
        error
            .to_string()
            .contains("cooling down after rate/usage/billing limit"),
        "{error}"
    );
    assert_eq!(request.model, CLINE_FLASH);
}

#[test]
fn outer_turn_is_not_rewritten_by_subagent_http_helper() {
    let root = tempfile::tempdir().expect("outer rewrite fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            false,
        )
        .expect("outer turns stay on preflight");
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn concurrency_admission_timeout_is_not_a_usage_limit_failover() {
    // TUI: `API Error: model \`qwen3.8-max-preview\` concurrency model admission
    // timed out after 29.999999375s`. That is capacity, not billing; do not cool
    // down Qwen or jump the outer turn onto Subscription.
    let error = anyhow::anyhow!(
        "model `qwen3.8-max-preview` concurrency model admission timed out after 29.999999375s"
    );
    assert!(!should_failover_provider_error(&error));
    assert!(super::super::model_concurrency::is_concurrency_admission_timeout(&error));
}

#[test]
fn concurrency_preflight_skips_non_subagent_and_non_provider_routes() {
    let bridge = cline_and_qwen_bridge();
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    assert_eq!(
        bridge.apply_concurrency_preflight(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            false,
        ),
        RouteDecision::Provider
    );
    assert_eq!(request.model, QWEN_CLOUD);
    assert_eq!(
        bridge.apply_concurrency_preflight(
            &mut request,
            RouteDecision::Subscription,
            &mut effort,
            true,
        ),
        RouteDecision::Subscription
    );
}

#[tokio::test]
async fn saturated_model_without_sibling_stays_on_the_same_provider() {
    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp);
    qwen.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[qwen]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-qwen",
            QWEN_CLOUD,
            "high",
        )])
        .expect("qwen worker");
    let bridge =
        Bridge::new_with_backend(backend, QWEN_CLOUD.to_owned()).with_model_catalog(catalog);
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let route = bridge.apply_concurrency_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        true,
    );
    assert_eq!(request.model, QWEN_CLOUD);
    assert_eq!(route, RouteDecision::Provider);
    assert!(
        bridge
            .reticket_after_concurrency_timeout(&mut request, &mut effort)
            .is_none()
    );
}

#[tokio::test]
async fn saturated_qwen_subagent_preflight_moves_to_a_free_sibling() {
    let bridge = cline_qwen_cursor_bridge();
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let route = bridge.apply_concurrency_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        true,
    );
    // Remaining ACP candidates are alphabetical after preferred Qwen, so
    // `auto` (Cursor) is chosen before Cline when Qwen slots are full.
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

#[tokio::test]
async fn cline_cooldown_plus_saturated_qwen_rewrites_onto_cursor() {
    // Horse-racing TUI: Cline empty-ACP failovers piled onto Qwen until
    // admission timed out. Sibling selection must skip both exhausted Cline
    // and slot-saturated Qwen.
    let root = tempfile::tempdir().expect("capacity fixture");
    let bridge = cline_qwen_cursor_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let _permits = saturate_qwen_subagent_slots(&bridge).await;

    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("sibling rewrite");
    let route = bridge.apply_concurrency_preflight(&mut request, route, &mut effort, true);

    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

#[tokio::test]
async fn admission_timeout_retickets_onto_a_free_sibling() {
    let bridge = cline_qwen_cursor_bridge();
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let ticket = bridge
        .reticket_after_concurrency_timeout(&mut request, &mut effort)
        .expect("sibling reticket");
    assert!(
        ticket.is_none(),
        "Cursor has no maxConcurrency, so the sibling starts without a ticket"
    );
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
}

/// Exact TUI wording from fa522331 multi-SubAgent launch onto Qwen.
const TUI_QWEN_TOKEN_PLAN: &str = "API Error: 502 codex app-server turn failed: \
Configured ACP prompt failed: Quota exhausted: Your token-plan 1-week quota has been exhausted. \
The quota will reset at 08-15 01:53:00 UTC.";

#[test]
fn treats_qwen_token_plan_quota_as_failover_trigger() {
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        TUI_QWEN_TOKEN_PLAN
    )));
    assert!(
        !super::super::stream::usage_limit::contains_classic_usage_limit_marker(
            TUI_QWEN_TOKEN_PLAN
        ),
        "must not classify Qwen token-plan as Codex app-server usage limit"
    );
}

#[test]
fn records_qwen_token_plan_cooldown_without_codex_usage_limit() {
    use std::time::SystemTime;

    use crate::anthropic::{provider_auth_cooldown, usage_limit_cooldown};

    let root = tempfile::tempdir().expect("qwen quota fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(QWEN_CLOUD));
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_QWEN_TOKEN_PLAN), Some(QWEN_CLOUD));
    assert!(
        bridge.subagent_provider_is_exhausted(QWEN_CLOUD),
        "Qwen token-plan exhaustion must cool down that SubAgent provider"
    );
    assert!(provider_auth_cooldown::scope_is_cooling_down_at(
        bridge.provider_auth_cache_path().as_deref(),
        QWEN_CLOUD,
        SystemTime::now(),
    ));
    assert!(
        !bridge.subagent_provider_is_exhausted(CLINE_FLASH),
        "Qwen quota must not cool down unrelated Cline"
    );
    assert!(!usage_limit_cooldown::codex_app_server_is_cooling_down_at(
        bridge.usage_limit_cache_path().as_deref(),
        SystemTime::now(),
    ));
}

#[test]
fn multi_subagent_rewrite_skips_qwen_after_token_plan_quota() {
    // Historical bug: Cline empty-ACP failovers always preferred Qwen, even after
    // Qwen's token-plan was exhausted, so parallel SubAgent launches 502'd.
    let root = tempfile::tempdir().expect("multi-subagent quota fixture");
    let bridge = cline_qwen_cursor_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_QWEN_TOKEN_PLAN), Some(QWEN_CLOUD));

    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": CLINE_FLASH,
        "claudex_effort": "xhigh",
        "prompt": "continue after quota"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["subagent_type"], "claudex-cursor");
    assert_eq!(arguments["claudex_model"], CURSOR_AUTO);
    assert_eq!(arguments["claudex_effort"], "high");

    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("remaining sibling");
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);

    let retry = streaming_provider_retry(bridge.failover_for_stream_turn(QWEN_CLOUD, true))
        .expect("explicit Qwen SubAgent must leave exhausted token-plan");
    assert_eq!(retry.model, CURSOR_AUTO);
}

#[test]
fn explicit_qwen_subagent_is_rewritten_after_token_plan_quota() {
    let root = tempfile::tempdir().expect("explicit qwen fixture");
    let bridge = cline_qwen_cursor_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_QWEN_TOKEN_PLAN), Some(QWEN_CLOUD));
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("sibling rewrite");
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn outer_turn_is_not_rewritten_by_concurrency_preflight() {
    let bridge = cline_qwen_cursor_bridge();
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let route = bridge.apply_concurrency_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        false,
    );
    assert_eq!(request.model, QWEN_CLOUD);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

fn write_usage_routing_qwen_exhausted(home: &std::path::Path) {
    let dir = home.join(".cache/claudex");
    std::fs::create_dir_all(&dir).expect("usage-routing dir");
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs_f64();
    let body = serde_json::json!({
        "created_at": created,
        "configuration_key": "test",
        "summary": {
            "providers": {
                "qwen": {
                    "available": false,
                    "reason": "exhausted",
                    "model": QWEN_CLOUD,
                    "agent": "claudex-qwen"
                },
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": CURSOR_AUTO,
                    "agent": "claudex-cursor"
                }
            },
            "selected_workers": [{
                "agent": "claudex-cursor",
                "model": CURSOR_AUTO,
                "effort": "high"
            }],
            "disabled_subagent_models": [QWEN_CLOUD]
        }
    });
    std::fs::write(
        dir.join("usage-routing.json"),
        serde_json::to_vec(&body).expect("usage-routing json"),
    )
    .expect("write usage-routing");
}

fn qwen_exhausted_routing_messages() -> Vec<Value> {
    vec![serde_json::json!({
        "role": "user",
        "content": format!(
            r#"Claudex routing for this turn: {{"providers":{{"qwen":{{"available":false,"reason":"exhausted","model":"{QWEN_CLOUD}"}}}},"selected_workers":[{{"agent":"claudex-cursor","model":"{CURSOR_AUTO}","effort":"high"}}],"disabled_subagent_models":["{QWEN_CLOUD}"]}}"#
        )
    })]
}

#[test]
fn usage_routing_quota_skips_qwen_on_multi_subagent_generation() {
    // Historical bug: CodexBar/route-usage already marked Qwen token-plan
    // exhausted, but Cline empty-ACP failover still preferred Qwen so parallel
    // SubAgent launches 502'd before any cooldown file existed.
    let root = tempfile::tempdir().expect("usage-routing quota fixture");
    write_usage_routing_qwen_exhausted(root.path());
    let bridge = cline_qwen_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        bridge.subagent_provider_is_exhausted(QWEN_CLOUD),
        "live usage-routing quota must cool down Qwen before any ACP 502"
    );
    assert!(!bridge.subagent_provider_is_exhausted(CLINE_FLASH));

    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let failover = bridge
        .subagent_provider_failover_for(CLINE_FLASH)
        .expect("remaining sibling");
    assert_eq!(failover.model, CURSOR_AUTO);

    let mut arguments = serde_json::json!({
        "subagent_type": "general-purpose",
        "claudex_model": CLINE_FLASH,
        "claudex_effort": "xhigh",
        "prompt": "parallel after quota view"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["subagent_type"], "claudex-cursor");
    assert_eq!(arguments["claudex_model"], CURSOR_AUTO);
    assert_eq!(arguments["claudex_effort"], "high");
}

#[test]
fn prompt_snapshot_quota_rewrites_explicit_qwen_without_cooldown() {
    let root = tempfile::tempdir().expect("prompt quota fixture");
    let bridge = cline_qwen_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        !bridge.subagent_provider_is_exhausted(QWEN_CLOUD),
        "no cooldown file and no usage-routing cache"
    );

    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-qwen",
        "claudex_model": QWEN_CLOUD,
        "claudex_effort": "high",
        "prompt": "explicit qwen after quota snapshot"
    });
    let messages = qwen_exhausted_routing_messages();
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &messages, &Value::Null);
    assert_eq!(arguments["subagent_type"], "claudex-cursor");
    assert_eq!(arguments["claudex_model"], CURSOR_AUTO);

    let mut request = dummy_request(QWEN_CLOUD);
    request.messages = qwen_exhausted_routing_messages();
    let mut effort = Some("high".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("snapshot rewrite");
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn should_failover_provider_error_returns_false_for_non_failover_errors() {
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Some generic error"
    )));
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Connection timeout"
    )));
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Model not found"
    )));
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "Invalid request format"
    )));
}

#[test]
fn should_failover_provider_error_returns_true_for_usage_limit() {
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        "codex app-server turn failed: exceeded retry limit, last status: 429 Too Many Requests"
    )));
}

#[test]
fn should_failover_provider_error_returns_true_for_auth_failure() {
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        "codex app-server turn failed: unexpected status 401 Unauthorized: Invalid API key"
    )));
}

#[test]
fn should_failover_provider_error_returns_true_for_empty_acp_billing() {
    assert!(should_failover_provider_error(&anyhow::anyhow!(
        EMPTY_ACP_END_TURN
    )));
}

#[test]
fn streaming_provider_retry_returns_none_for_subscription_failover() {
    let subscription_failover = Some(UsageLimitFailover {
        model: "claude-opus-5".to_owned(),
        effort: Some("high".to_owned()),
        route: RouteDecision::Subscription,
    });
    assert!(streaming_provider_retry(subscription_failover).is_none());
}

#[test]
fn streaming_provider_retry_returns_some_for_provider_failover() {
    let provider_failover = Some(UsageLimitFailover {
        model: QWEN_CLOUD.to_owned(),
        effort: Some("high".to_owned()),
        route: RouteDecision::Provider,
    });
    let retry = streaming_provider_retry(provider_failover).expect("provider should stream-retry");
    assert_eq!(retry.model, QWEN_CLOUD);
    assert_eq!(retry.route, RouteDecision::Provider);
}

#[test]
fn streaming_provider_retry_returns_none_for_none_failover() {
    assert!(streaming_provider_retry(None).is_none());
}

#[test]
fn subagent_provider_failover_for_non_acp_returns_none() {
    let backend = AgentBackend::spawn_routes(&[BackendRoute::new(
        "claude-opus-5",
        BackendKind::CodexAppServer,
    )]);
    let bridge = Bridge::new_with_backend(backend, "claude-opus-5".to_owned());
    assert!(
        bridge
            .subagent_provider_failover_for("claude-opus-5")
            .is_none()
    );
}

#[test]
fn usage_limit_failover_for_returns_configured_fallback() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::ConfiguredAcp)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install fallback");
    let bridge =
        Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog);
    let failover = bridge
        .usage_limit_failover_for(CLINE_FLASH)
        .expect("fallback target");
    assert_eq!(failover.model, "claude-sonnet-5");
    assert_eq!(failover.effort.as_deref(), Some("high"));
}

#[test]
fn failover_for_stream_turn_subagent_true_uses_provider_then_subscription() {
    let bridge = cline_and_qwen_bridge();
    let sibling = bridge
        .failover_for_stream_turn(CLINE_FLASH, true)
        .expect("subagent stream failover");
    assert_eq!(sibling.route, RouteDecision::Provider);
}

#[test]
fn failover_for_stream_turn_subagent_false_uses_subscription() {
    let bridge = cline_and_qwen_bridge();
    let subscription = bridge
        .failover_for_stream_turn(CLINE_FLASH, false)
        .expect("outer stream failover");
    assert_eq!(subscription.route, RouteDecision::Subscription);
}

#[test]
fn model_uses_codex_app_server_true_branch() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new("fugu", BackendKind::CodexAppServer)]);
    let bridge = Bridge::new_with_backend(backend, "fugu".to_owned());
    assert!(bridge.model_uses_codex_app_server("fugu"));
}

#[test]
fn model_uses_codex_app_server_false_branch() {
    let bridge = cline_and_qwen_bridge();
    assert!(!bridge.model_uses_codex_app_server(CLINE_FLASH));
    assert!(!bridge.model_uses_codex_app_server(QWEN_CLOUD));
}

#[test]
fn apply_usage_limit_preflight_skips_subagent() {
    let bridge = cline_and_qwen_bridge();
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = None;
    let route = bridge.apply_usage_limit_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        true,
    );
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn apply_usage_limit_preflight_skips_non_provider_route() {
    let bridge = cline_and_qwen_bridge();
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = None;
    let route = bridge.apply_usage_limit_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
        false,
    );
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(route, RouteDecision::Subscription);
}

#[test]
fn apply_usage_limit_preflight_activates_when_auth_cooling_down() {
    let root = tempfile::tempdir().expect("preflight auth fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("codex app-server turn failed: 401 Unauthorized"),
        Some(CLINE_FLASH),
    );
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge.apply_usage_limit_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        false,
    );
    assert_eq!(request.model, "claude-sonnet-5");
    assert_eq!(route, RouteDecision::Subscription);
}

#[test]
fn provider_auth_is_cooling_down_true() {
    let root = tempfile::tempdir().expect("auth cooling fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("codex app-server turn failed: 401 Unauthorized"),
        Some(CLINE_FLASH),
    );
    assert!(bridge.provider_auth_is_cooling_down(CLINE_FLASH));
    assert!(!bridge.provider_auth_is_cooling_down(QWEN_CLOUD));
}

#[test]
fn codex_usage_limit_is_active_true() {
    let root = tempfile::tempdir().expect("codex limit fixture");
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new("fugu", BackendKind::CodexAppServer),
        BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp),
    ]);
    let bridge = Bridge::new_with_backend(backend, "fugu".to_owned())
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("You've hit your usage limit."),
        Some("fugu"),
    );
    assert!(bridge.codex_usage_limit_is_active("fugu"));
    assert!(!bridge.codex_usage_limit_is_active(QWEN_CLOUD));
}

fn spark_luna_cursor_bridge() -> Bridge {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(SPARK, BackendKind::CodexAppServer),
        BackendRoute::new(LUNA, BackendKind::CodexAppServer),
        BackendRoute::new(CURSOR_AUTO, BackendKind::ConfiguredAcp),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-gpt-spark", SPARK, "xhigh"),
            crate::provider_config::WorkerRoute::new("claudex-gpt", LUNA, "max"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", CURSOR_AUTO, "high"),
        ])
        .expect("install spark workers");
    Bridge::new_with_backend(backend, SPARK.to_owned()).with_model_catalog(catalog)
}

fn write_usage_routing_spark_low_remaining(home: &std::path::Path) {
    let dir = home.join(".cache/claudex");
    std::fs::create_dir_all(&dir).expect("usage-routing dir");
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs_f64();
    let body = serde_json::json!({
        "created_at": created,
        "configuration_key": "test",
        "summary": {
            "providers": {
                "codex-spark": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": SPARK,
                    "remaining_percent": 17.0,
                    "quota_windows": {"five-hour": null, "seven-day": 17.0}
                },
                "codex": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": LUNA,
                    "remaining_percent": 98.0,
                    "quota_windows": {"five-hour": null, "seven-day": 98.0}
                },
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": CURSOR_AUTO,
                    "remaining_percent": 99.9
                }
            },
            "selected_workers": [{
                "agent": "claudex-cursor",
                "model": CURSOR_AUTO,
                "effort": "high"
            }],
            "disabled_subagent_models": []
        }
    });
    std::fs::write(
        dir.join("usage-routing.json"),
        serde_json::to_vec(&body).expect("usage-routing json"),
    )
    .expect("write usage-routing");
}

#[test]
fn low_remaining_spark_is_not_exhausted_without_usage_snapshot() {
    let root = tempfile::tempdir().expect("spark no snapshot fixture");
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        !bridge.subagent_provider_is_exhausted(SPARK),
        "old behavior: spark stays launchable when CodexBar still says available"
    );
}

#[test]
fn low_remaining_spark_launch_rewrites_onto_cursor() {
    // Historical TUI bug: automatic selected_workers already dropped spark at
    // 17% weekly remaining, but explicit `claudex-gpt-spark` kept starting.
    let root = tempfile::tempdir().expect("spark low remaining fixture");
    write_usage_routing_spark_low_remaining(root.path());
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        bridge.subagent_provider_is_exhausted(SPARK),
        "live usage-routing low remaining must cool down spark before another launch"
    );
    assert!(!bridge.subagent_provider_is_exhausted(LUNA));
    assert!(!bridge.subagent_provider_is_exhausted(CURSOR_AUTO));

    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-gpt-spark",
        "claudex_model": SPARK,
        "claudex_effort": "xhigh",
        "prompt": "continue after low spark quota"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["subagent_type"], "claudex-cursor");
    assert_eq!(arguments["claudex_model"], CURSOR_AUTO);
    assert_eq!(arguments["claudex_effort"], "high");
}

#[test]
fn low_remaining_spark_http_subagent_rewrites_onto_cursor() {
    let root = tempfile::tempdir().expect("spark http rewrite fixture");
    write_usage_routing_spark_low_remaining(root.path());
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    let mut request = dummy_request(SPARK);
    let mut effort = Some("xhigh".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("spark must leave onto an ACP sibling");
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(effort.as_deref(), Some("high"));
    assert_eq!(route, RouteDecision::Provider);
}
