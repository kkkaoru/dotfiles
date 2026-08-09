use serde_json::Value;

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::request_routing::RouteDecision;
use crate::anthropic::{Bridge, MessagesRequest};
use crate::provider_config::ModelCatalog;

use super::super::segment::EMPTY_ACP_END_TURN;
use super::{should_failover_provider_error, streaming_provider_retry};

const CLINE_FLASH: &str = "cline-pass/deepseek-v4-flash";
const QWEN_CLOUD: &str = "qwen3.8-max-preview";
const CURSOR_AUTO: &str = "auto";

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
    Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog)
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

    bridge.rewrite_exhausted_agent_launch(&mut arguments);

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
