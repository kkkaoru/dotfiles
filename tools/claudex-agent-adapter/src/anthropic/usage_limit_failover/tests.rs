use serde_json::Value;
use std::time::{Duration, SystemTime};

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};

use crate::anthropic::provider_auth_cooldown;
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
        BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway),
        BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway),
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
        BackendRoute::new("gpt-5.6-luna", BackendKind::CodexAppServer),
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
fn cline_credits_insufficient_balance_does_not_failover() {
    assert!(!should_failover_provider_error(&anyhow::anyhow!(
        "ConfiguredLaunch ACP prompt failed: Internal error: Insufficient balance. \
Add credits at https://app.cline.bot/credits or retry with a different model."
    )));
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

fn cline_qwen_cursor_bridge() -> Bridge {
    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway);
    qwen.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway),
        qwen,
        BackendRoute::new(CURSOR_AUTO, BackendKind::PiGateway),
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
            .contains("cooling down after a rate/usage/billing limit"),
        "{error}"
    );
    assert_eq!(request.model, "glm-5.2:cloud");
}

#[test]
fn exhausted_subagent_without_sibling_still_rejects() {
    let root = tempfile::tempdir().expect("no sibling fixture");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
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
            .contains("cooling down after a rate/usage/billing limit"),
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
    let (route, routing_summary_searches) =
        super::super::agent_routing::count_routing_summary_searches(|| {
            bridge
                .rewrite_exhausted_subagent_request(
                    &mut request,
                    RouteDecision::Provider,
                    &mut effort,
                    false,
                )
                .expect("outer turns stay on preflight")
        });
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    assert_eq!(route, RouteDecision::Provider);
    assert_eq!(
        routing_summary_searches, 0,
        "outer turns must skip the SubAgent-only routing-summary history search"
    );
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
    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway);
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

const TUI_OPENCODE_WEEKLY: &str = "AI_APICallError: Weekly usage limit reached. Resets in 4 days.";

const OPENCODE_FLASH: &str = "opencode-go/deepseek-v4-flash";

fn opencode_and_codex_bridge() -> Bridge {
    let cache_home = Box::leak(Box::new(
        tempfile::tempdir().expect("isolated opencode cache"),
    ));
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(OPENCODE_FLASH, BackendKind::PiGateway),
        BackendRoute::new(SPARK, BackendKind::CodexAppServer),
    ]);
    Bridge::new_with_backend(backend, OPENCODE_FLASH.to_owned())
        .with_usage_limit_cache_home(cache_home.path())
}

#[test]
fn records_opencode_weekly_quota_without_codex_usage_limit() {
    use std::time::SystemTime;

    use crate::anthropic::{provider_auth_cooldown, usage_limit_cooldown};

    let root = tempfile::tempdir().expect("opencode quota fixture");
    let bridge = opencode_and_codex_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(OPENCODE_FLASH));
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_OPENCODE_WEEKLY), Some(OPENCODE_FLASH));
    assert!(
        bridge.subagent_provider_is_exhausted(OPENCODE_FLASH),
        "OpenCode weekly cap must cool down that provider"
    );
    assert!(provider_auth_cooldown::scope_is_cooling_down_at(
        bridge.provider_auth_cache_path().as_deref(),
        OPENCODE_FLASH,
        SystemTime::now(),
    ));
    assert!(
        !bridge.subagent_provider_is_exhausted(SPARK),
        "OpenCode weekly cap must not cool Codex"
    );
    assert!(!usage_limit_cooldown::codex_app_server_is_cooling_down_at(
        bridge.usage_limit_cache_path().as_deref(),
        SystemTime::now(),
    ));
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
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
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
    let route = bridge
        .apply_usage_limit_preflight(&mut request, RouteDecision::Provider, &mut effort, true)
        .expect("subagent preflight");
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn apply_usage_limit_preflight_skips_non_provider_route() {
    let bridge = cline_and_qwen_bridge();
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = None;
    let route = bridge
        .apply_usage_limit_preflight(
            &mut request,
            RouteDecision::Subscription,
            &mut effort,
            false,
        )
        .expect("non-provider preflight");
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
    let error = bridge
        .apply_usage_limit_preflight(&mut request, RouteDecision::Provider, &mut effort, false)
        .expect_err("auth cooldown must not silently fail over");
    assert_eq!(request.model, CLINE_FLASH);
    assert!(error.to_string().contains(CLINE_FLASH));
    assert!(error.to_string().contains("auth"));
    assert!(error.to_string().contains("failover is disabled"));
}

#[test]
fn apply_usage_limit_preflight_rewrites_effort_from_configured_fallback() {
    let root = tempfile::tempdir().expect("preflight effort fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("codex app-server turn failed: 401 Unauthorized"),
        Some(CLINE_FLASH),
    );
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let error = bridge
        .apply_usage_limit_preflight(&mut request, RouteDecision::Provider, &mut effort, false)
        .expect_err("auth cooldown must keep the requested effort");
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    assert!(error.to_string().contains("failover is disabled"));
}

#[test]
fn apply_usage_limit_preflight_keeps_provider_when_cooling_down_without_failover() {
    let root = tempfile::tempdir().expect("preflight no failover fixture");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
    let bridge = Bridge::new_with_backend(backend, CLINE_FLASH.to_owned())
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("codex app-server turn failed: 401 Unauthorized"),
        Some(CLINE_FLASH),
    );
    let mut request = dummy_request(CLINE_FLASH);
    let mut effort = Some("xhigh".to_owned());
    let error = bridge
        .apply_usage_limit_preflight(&mut request, RouteDecision::Provider, &mut effort, false)
        .expect_err("cooldown without a target must still fail closed");
    assert_eq!(request.model, CLINE_FLASH);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    assert!(error.to_string().contains(CLINE_FLASH));
    assert!(error.to_string().contains("failover is disabled"));
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
        BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway),
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
        BackendRoute::new(CURSOR_AUTO, BackendKind::PiGateway),
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

fn write_usage_routing_cursor_auto_low_remaining(home: &std::path::Path) {
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
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": CURSOR_AUTO,
                    "remaining_percent": 9.1965
                },
                "grok": {
                    "available": true,
                    "reason": "available-grok-quota",
                    "model": "grok-4.6",
                    "remaining_percent": 66.0
                }
            },
            "selected_workers": [{
                "agent": "claudex-grok",
                "model": "grok-4.6",
                "effort": "medium"
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
fn low_remaining_spark_launch_stays_on_spark() {
    // Low remaining is a selection heuristic. Explicit spark launches must not
    // be rewritten or hard-blocked when CodexBar still says available.
    let root = tempfile::tempdir().expect("spark low remaining fixture");
    write_usage_routing_spark_low_remaining(root.path());
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(SPARK));
    assert!(!bridge.subagent_provider_is_exhausted(LUNA));
    assert!(!bridge.subagent_provider_is_exhausted(CURSOR_AUTO));

    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-gpt-spark",
        "claudex_model": SPARK,
        "claudex_effort": "xhigh",
        "prompt": "continue after low spark quota"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["subagent_type"], "claudex-gpt-spark");
    assert_eq!(arguments["claudex_model"], SPARK);
    assert_eq!(arguments["claudex_effort"], "xhigh");
}

#[test]
fn low_remaining_spark_http_subagent_stays_on_spark() {
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
        .expect("low remaining must not reject spark");
    assert_eq!(request.model, SPARK);
    assert_eq!(effort.as_deref(), Some("xhigh"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn low_remaining_cursor_auto_stays_launchable() {
    let root = tempfile::tempdir().expect("cursor auto low remaining fixture");
    write_usage_routing_cursor_auto_low_remaining(root.path());
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        !bridge.subagent_provider_is_exhausted(CURSOR_AUTO),
        "cursor auto with remaining quota must not cool down"
    );

    let mut arguments = serde_json::json!({
        "subagent_type": "claudex-cursor",
        "claudex_model": CURSOR_AUTO,
        "claudex_effort": "high",
        "prompt": "research with cursor auto"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["claudex_model"], CURSOR_AUTO);

    let mut request = dummy_request(CURSOR_AUTO);
    let mut effort = Some("high".to_owned());
    let route = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect("cursor auto must stay launchable");
    assert_eq!(request.model, CURSOR_AUTO);
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn disabled_subagent_model_without_sibling_hard_blocks() {
    let root = tempfile::tempdir().expect("disabled list fixture");
    let dir = root.path().join(".cache/claudex");
    std::fs::create_dir_all(&dir).expect("usage-routing dir");
    let created = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs_f64();
    let body = serde_json::json!({
        "created_at": created,
        "configuration_key": "test",
        "summary": {
            "providers": {
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": CURSOR_AUTO,
                    "remaining_percent": 9.1965
                }
            },
            "disabled_subagent_models": [CURSOR_AUTO]
        }
    });
    std::fs::write(
        dir.join("usage-routing.json"),
        serde_json::to_vec(&body).expect("usage-routing json"),
    )
    .expect("write usage-routing");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CURSOR_AUTO, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-cursor",
            CURSOR_AUTO,
            "high",
        )])
        .expect("install cursor only");
    let bridge = Bridge::new_with_backend(backend, CURSOR_AUTO.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    assert!(bridge.subagent_provider_is_exhausted(CURSOR_AUTO));
    let mut request = dummy_request(CURSOR_AUTO);
    let mut effort = Some("high".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("disabled auto without sibling must hard-block");
    assert!(
        error
            .to_string()
            .contains("cooling down after a rate/usage/billing limit"),
        "unexpected reject: {error}"
    );
    assert_eq!(request.model, CURSOR_AUTO);
}

#[test]
fn provider_exhaustion_cooldown_reason_hard_blocks_without_sibling() {
    let root = tempfile::tempdir().expect("provider-exhaustion-cooldown fixture");
    let dir = root.path().join(".cache/claudex");
    std::fs::create_dir_all(&dir).expect("usage-routing dir");
    let created = SystemTime::now()
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
                    "reason": "provider-exhaustion-cooldown",
                    "model": QWEN_CLOUD
                }
            },
            "disabled_subagent_models": []
        }
    });
    std::fs::write(
        dir.join("usage-routing.json"),
        serde_json::to_vec(&body).expect("usage-routing json"),
    )
    .expect("write usage-routing");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-qwen",
            QWEN_CLOUD,
            "high",
        )])
        .expect("install qwen only");
    let bridge = Bridge::new_with_backend(backend, QWEN_CLOUD.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    assert!(bridge.subagent_provider_is_exhausted(QWEN_CLOUD));
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("provider-exhaustion-cooldown without sibling must hard-block");
    assert!(
        error
            .to_string()
            .contains("cooling down after a rate/usage/billing limit")
    );
}

#[test]
fn active_auth_cooldown_hard_blocks_without_sibling() {
    let root = tempfile::tempdir().expect("active auth cooldown fixture");
    let path = provider_auth_cooldown::cache_path_for_home(root.path());
    let now = SystemTime::now();
    provider_auth_cooldown::record_at(Some(&path), CURSOR_AUTO, "401 Unauthorized", now)
        .expect("record active auth cooldown");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CURSOR_AUTO, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-cursor",
            CURSOR_AUTO,
            "high",
        )])
        .expect("install cursor only");
    let bridge = Bridge::new_with_backend(backend, CURSOR_AUTO.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    assert!(bridge.provider_auth_is_cooling_down(CURSOR_AUTO));
    assert!(bridge.subagent_provider_is_exhausted(CURSOR_AUTO));
    let mut request = dummy_request(CURSOR_AUTO);
    let mut effort = Some("high".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("active auth cooldown without sibling must hard-block");
    assert!(
        error
            .to_string()
            .contains("cooling down after a rate/usage/billing limit")
    );
}

#[test]
fn active_quota_cooldown_hard_blocks_without_sibling() {
    let root = tempfile::tempdir().expect("active quota cooldown fixture");
    let path = provider_auth_cooldown::cache_path_for_home(root.path());
    let now = SystemTime::now();
    provider_auth_cooldown::record_rate_limit_at(
        Some(&path),
        QWEN_CLOUD,
        "402 Payment Required: quota exhausted",
        now,
    )
    .expect("record active quota cooldown");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-qwen",
            QWEN_CLOUD,
            "high",
        )])
        .expect("install qwen only");
    let bridge = Bridge::new_with_backend(backend, QWEN_CLOUD.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    assert!(bridge.provider_auth_is_cooling_down(QWEN_CLOUD));
    assert!(bridge.subagent_provider_is_exhausted(QWEN_CLOUD));
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let error = bridge
        .rewrite_exhausted_subagent_request(
            &mut request,
            RouteDecision::Provider,
            &mut effort,
            true,
        )
        .expect_err("active quota cooldown without sibling must hard-block");
    assert!(
        error
            .to_string()
            .contains("cooling down after a rate/usage/billing limit")
    );
}

#[test]
fn auth_cooldown_boundary_blocks_until_and_releases_at_expiry() {
    let root = tempfile::tempdir().expect("auth cooldown expiry fixture");
    let path = provider_auth_cooldown::cache_path_for_home(root.path());
    let now = SystemTime::now();
    let until = now
        .duration_since(std::time::UNIX_EPOCH)
        .expect("epoch")
        .as_secs()
        + 30;
    std::fs::create_dir_all(path.parent().expect("cache parent")).expect("cache dir");
    let cache = serde_json::json!({
        "version": 1,
        "entries": {
            "auto": {
                "untilUnixSeconds": until,
                "message": "401 Unauthorized",
                "recordedUnixSeconds": until - 30
            }
        }
    });
    std::fs::write(&path, serde_json::to_vec(&cache).expect("json")).expect("write");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CURSOR_AUTO, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-cursor",
            CURSOR_AUTO,
            "high",
        )])
        .expect("install cursor only");
    let bridge = Bridge::new_with_backend(backend, CURSOR_AUTO.to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    assert!(
        provider_auth_cooldown::scope_is_cooling_down_at(
            Some(&path),
            CURSOR_AUTO,
            std::time::UNIX_EPOCH + Duration::from_secs(until - 1),
        ),
        "one second before expiry must still block"
    );
    assert!(
        !provider_auth_cooldown::scope_is_cooling_down_at(
            Some(&path),
            CURSOR_AUTO,
            std::time::UNIX_EPOCH + Duration::from_secs(until),
        ),
        "exactly at untilUnixSeconds must already be expired"
    );
    assert!(bridge.subagent_provider_is_exhausted(CURSOR_AUTO));
}

#[test]
fn expired_provider_auth_cooldown_is_ignored() {
    let root = tempfile::tempdir().expect("expired cooldown fixture");
    let path = provider_auth_cooldown::cache_path_for_home(root.path());
    let now = SystemTime::now();
    let expired = now
        .checked_sub(Duration::from_secs(60))
        .expect("expired timestamp");
    provider_auth_cooldown::record_rate_limit_at(
        Some(&path),
        "grok-4.6",
        "402 Payment Required: usage balance exhausted",
        expired
            .checked_sub(Duration::from_secs(4 * 60 * 60))
            .expect("recorded in the past"),
    )
    .expect("record expired cooldown");
    // Force the entry to an already-elapsed until by rewriting the cache.
    let mut cache: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read cooldown"))
            .expect("parse cooldown");
    cache["entries"]["grok-4.6"]["untilUnixSeconds"] = serde_json::json!(
        expired
            .duration_since(std::time::UNIX_EPOCH)
            .expect("epoch")
            .as_secs()
    );
    std::fs::write(&path, serde_json::to_vec(&cache).expect("json")).expect("write");
    assert!(!provider_auth_cooldown::scope_is_cooling_down_at(
        Some(&path),
        "grok-4.6",
        now
    ));
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted("grok-4.6"));
    assert!(!bridge.subagent_provider_is_exhausted(CURSOR_AUTO));
}

#[test]
fn note_provider_exhaustion_skips_superseded_cursor_request() {
    let root = tempfile::tempdir().expect("superseded cursor fixture");
    let bridge = spark_luna_cursor_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(CURSOR_AUTO));
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!(
            "codex app-server turn failed: {{\"error\":{{\"message\":\"Superseded by a new Cursor request\"}}}}"
        ),
        Some(CURSOR_AUTO),
    );
    assert!(
        !bridge.subagent_provider_is_exhausted(CURSOR_AUTO),
        "Superseded is abort/replace, not rate/usage/billing exhaustion"
    );
}

#[test]
fn note_provider_exhaustion_skips_non_failure_errors() {
    let root = tempfile::tempdir().expect("note-exhaustion unrelated error fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subagent_provider_is_exhausted(CLINE_FLASH));
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("some generic network error"),
        Some(CLINE_FLASH),
    );
    assert!(
        !bridge.subagent_provider_is_exhausted(CLINE_FLASH),
        "unrelated error must not trigger cooldown"
    );
}

#[test]
fn usage_limit_failover_for_with_no_configured_fallback() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
    let bridge = Bridge::new_with_backend(backend, CLINE_FLASH.to_owned());
    let failover = bridge.usage_limit_failover_for(CLINE_FLASH);
    assert!(
        failover.is_none(),
        "no failover target when no auxiliary fallback configured"
    );
}

#[test]
fn auth_scopes_for_model_none_and_empty_message() {
    let bridge = cline_and_qwen_bridge();
    let scopes = bridge.auth_scopes_for(None, "");
    assert!(
        scopes.is_empty(),
        "no scopes when model is None and message is empty"
    );
}

#[test]
fn auth_scopes_for_model_none_with_sakana_message() {
    let bridge = cline_and_qwen_bridge();
    let message_with_sakana = "API Error: 401 sakana api key failure";
    let scopes = bridge.auth_scopes_for(None, message_with_sakana);
    assert!(
        scopes.iter().any(|s| s == "sakana"),
        "scope must extract sakana from message even when model is None"
    );
}

#[test]
fn model_uses_codex_app_server_with_none_backend_kind() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "unknown-model".to_owned());
    let result = bridge.model_uses_codex_app_server("unknown-model");
    assert!(
        !result,
        "model with no backend kind should return false (not default to codex)"
    );
}

#[test]
fn model_uses_codex_app_server_codex_backend() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new("fugu", BackendKind::CodexAppServer)]);
    let bridge = Bridge::new_with_backend(backend, "fugu".to_owned());
    assert!(bridge.model_uses_codex_app_server("fugu"));
}

#[test]
fn model_uses_codex_app_server_non_codex_backend() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
    let bridge = Bridge::new_with_backend(backend, CLINE_FLASH.to_owned());
    assert!(!bridge.model_uses_codex_app_server(CLINE_FLASH));
}

#[test]
fn subagent_provider_failover_excluding_with_non_ok_target_kind() {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway),
        BackendRoute::new("codex", BackendKind::CodexAppServer),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-cline-deepseek-flash",
            CLINE_FLASH,
            "xhigh",
        )])
        .expect("install cline worker");
    let bridge =
        Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog);
    let failover = bridge.subagent_provider_failover_excluding(CLINE_FLASH, None);
    assert!(
        failover.is_none(),
        "must skip exhausted model and CodexAppServer (non-ok target), returning None"
    );
}

#[test]
fn failover_for_stream_turn_outer_uses_subscription_fallback() {
    let bridge = cline_and_qwen_bridge();
    let failover = bridge.failover_for_stream_turn(CLINE_FLASH, false);
    assert_eq!(
        failover.as_ref().map(|f| f.route),
        Some(RouteDecision::Subscription),
        "outer stream must use subscription fallback"
    );
}

#[test]
fn subagent_failover_is_none_when_no_provider_route_and_no_fallback() {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway),
        BackendRoute::new(QWEN_CLOUD, BackendKind::PiGateway),
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
    let bridge =
        Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog);
    let root = tempfile::tempdir().expect("no-fallback fixture");
    let bridge = bridge.with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(QWEN_CLOUD));
    let failover = bridge.failover_for_stream_turn(CLINE_FLASH, true);
    assert!(
        failover.is_none(),
        "subagent stream failover is None when all sibling providers exhausted and no fallback"
    );
}

#[test]
fn note_provider_exhaustion_without_cache_home_does_not_record() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]),
        CLINE_FLASH.to_owned(),
    );
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("You've hit your usage limit. Try again later."),
        Some(CLINE_FLASH),
    );
    bridge.note_provider_exhaustion(
        &anyhow::anyhow!("Please run /login · OAuth session expired and could not be refreshed"),
        Some("claude-opus-5"),
    );
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    assert!(
        !bridge.subagent_provider_is_exhausted(CLINE_FLASH),
        "without a cache home, exhaustion must not persist a cooldown"
    );
}

#[test]
fn rewrite_exhausted_launch_ignores_non_object_and_fresh_models() {
    let root = tempfile::tempdir().expect("rewrite noop fixture");
    let bridge = cline_and_qwen_bridge().with_usage_limit_cache_home(root.path());
    let mut missing_model = serde_json::json!({"prompt": "no model key"});
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut missing_model, &[], &Value::Null);
    assert_eq!(missing_model["prompt"], "no model key");

    let mut fresh = serde_json::json!({
        "claudex_model": CLINE_FLASH,
        "claudex_effort": "xhigh"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut fresh, &[], &Value::Null);
    assert_eq!(fresh["claudex_model"], CLINE_FLASH);

    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut not_object = serde_json::json!(["not-an-object"]);
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut not_object, &[], &Value::Null);
    assert_eq!(not_object, serde_json::json!(["not-an-object"]));
}

#[test]
fn rewrite_exhausted_launch_without_sibling_keeps_original() {
    let root = tempfile::tempdir().expect("no sibling fixture");
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
    let bridge = Bridge::new_with_backend(backend, CLINE_FLASH.to_owned())
        .with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(EMPTY_ACP_END_TURN), Some(CLINE_FLASH));
    let mut arguments = serde_json::json!({
        "claudex_model": CLINE_FLASH,
        "claudex_effort": "xhigh"
    });
    bridge.rewrite_exhausted_agent_launch_with_quota(&mut arguments, &[], &Value::Null);
    assert_eq!(arguments["claudex_model"], CLINE_FLASH);
}

#[tokio::test]
async fn provider_messages_failover_keeps_usage_limit_when_no_target_is_configured() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let root = tempfile::tempdir().expect("failover no-target fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let program = root.path().join("app-server");
    std::fs::write(
        &program,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"thread/start"'*)
      printf '{"id":%s,"error":{"message":"You'\''ve hit your usage limit."}}\n' "$id"
      ;;
    *)
      printf '{"id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .expect("usage-limit app-server");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("executable app-server");
    let app = crate::app_server::AppServer::spawn_with_program(
        "main",
        &program,
        &source,
        &root.path().join("isolated"),
    )
    .await
    .expect("start usage-limit app-server");
    let bridge = Arc::new(
        Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned())
            .with_usage_limit_cache_home(root.path()),
    );
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![serde_json::json!({"role":"user","content":"ping"})],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bridge.provider_messages_with_usage_limit_failover(request, None, false, false, false),
    )
    .await
    .expect("failover without target should not hang")
    .expect_err("usage-limit without failover target must stay failed");
    assert!(
        error.to_string().contains("failover is disabled"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("usage limit"), "{error:#}");
}

#[tokio::test]
async fn provider_messages_failover_attempts_configured_subscription_target() {
    let root = tempfile::tempdir().expect("failover subscription fixture");
    let (bridge, request) = failover_subscription_bridge_and_request(root.path()).await;
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        bridge.provider_messages_with_usage_limit_failover(
            request,
            Some("xhigh".to_owned()),
            false,
            false,
            false,
        ),
    )
    .await
    .expect("subscription failover should not hang")
    .expect_err("usage-limit must fail closed without switching models");
    assert!(
        error.to_string().contains("failover is disabled"),
        "{error:#}"
    );
}

async fn failover_subscription_bridge_and_request(
    root: &std::path::Path,
) -> (std::sync::Arc<Bridge>, MessagesRequest) {
    use std::os::unix::fs::PermissionsExt;

    let source = root.join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let app_server = root.join("app-server");
    std::fs::write(
        &app_server,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  [ -n "$id" ] || id=0
  case "$line" in
    *'"method":"thread/start"'*)
      printf '{"id":%s,"error":{"message":"You'\''ve hit your usage limit."}}\n' "$id"
      ;;
    *)
      printf '{"id":%s,"result":{}}\n' "$id"
      ;;
  esac
done
"#,
    )
    .expect("usage-limit app-server");
    std::fs::set_permissions(&app_server, std::fs::Permissions::from_mode(0o755))
        .expect("executable app-server");
    let claude = root.join("claude-fail");
    std::fs::write(
        &claude,
        r#"#!/bin/sh
printf '%s\n' '{"type":"result","subtype":"error","is_error":true,"result":"boom"}'
exit 1
"#,
    )
    .expect("write failing claude");
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))
        .expect("executable claude");
    let app = crate::app_server::AppServer::spawn_with_program(
        "main",
        &app_server,
        &source,
        &root.join("isolated"),
    )
    .await
    .expect("start usage-limit app-server");
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription failover");
    let bridge = std::sync::Arc::new(
        Bridge::new_with_subscription_program(app, "main".to_owned(), claude)
            .with_model_catalog(catalog)
            .with_usage_limit_cache_home(root),
    );
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![serde_json::json!({"role":"user","content":"ping"})],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    (bridge, request)
}

#[test]
fn usage_limit_failover_for_returns_subscription_fallback() {
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new(CLINE_FLASH, BackendKind::PiGateway)]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            LUNA,
            "high",
        )])
        .expect("install fallback");
    let bridge =
        Bridge::new_with_backend(backend, CLINE_FLASH.to_owned()).with_model_catalog(catalog);
    let failover = bridge
        .usage_limit_failover_for(CLINE_FLASH)
        .expect("configured subscription fallback");
    assert_eq!(failover.model, LUNA);
    assert_eq!(failover.route, RouteDecision::Subscription);
    assert_eq!(failover.effort, Some("high".to_owned()));
}
