use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::request_routing::RouteDecision;
use crate::anthropic::{Bridge, MessagesRequest};
use crate::app_server::AppServer;
use crate::provider_config::ModelCatalog;

use super::{SUBSCRIPTION_AUTH_SCOPE, credentials_access_expired_at, is_subscription_auth_failure};

/// Exact Claude Code / TUI wording from horse-racing `fa522331-…`.
const TUI_OAUTH: &str = "Please run /login · API Error: 401 Claude subscription model \
claude-opus-5 failed [authentication; exit status: 1; trace=2d31ef8d8e4d4288ae0b68c99c2eb18b]: \
Failed to authenticate: OAuth session expired and could not be refreshed";
/// Later resume of the same session before credentials were reconciled.
const TUI_OAUTH_RESUME: &str = "Please run /login · API Error: 401 Claude subscription model \
claude-opus-5 failed [authentication; exit status: 1; trace=7210210d09a547d8909f1f419d4b9d04]: \
Failed to authenticate: OAuth session expired and could not be refreshed";
const PROVIDER_401: &str =
    "unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses";

fn dummy_request(model: &str) -> MessagesRequest {
    MessagesRequest {
        model: model.to_owned(),
        system: serde_json::Value::Null,
        messages: Vec::new(),
        tools: Vec::new(),
        stream: true,
        output_config: serde_json::Value::Null,
        metadata: serde_json::Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn opus_luna_and_cursor_bridge() -> Bridge {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new("gpt-5.6-luna", BackendKind::CodexAppServer),
        BackendRoute::new("auto", BackendKind::ConfiguredAcp),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-gpt", "gpt-5.6-luna", "max"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", "auto", "high"),
        ])
        .expect("install luna and cursor workers");
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    Bridge::new_with_backend(backend, "gpt-5.6-luna".to_owned()).with_model_catalog(catalog)
}

fn fugu_luna_bridge() -> Bridge {
    let backend = AgentBackend::spawn_routes(&[
        BackendRoute::new("fugu", BackendKind::CodexAppServer),
        BackendRoute::new("gpt-5.6-luna", BackendKind::CodexAppServer),
    ]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-fugu", "fugu", "high"),
            crate::provider_config::WorkerRoute::new("claudex-gpt", "gpt-5.6-luna", "max"),
        ])
        .expect("install fugu and luna workers");
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog)
}

fn opus_and_luna_bridge() -> Bridge {
    let backend = AgentBackend::spawn_routes(&[BackendRoute::new(
        "gpt-5.6-luna",
        BackendKind::CodexAppServer,
    )]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-gpt",
            "gpt-5.6-luna",
            "max",
        )])
        .expect("install luna worker");
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    Bridge::new_with_backend(backend, "gpt-5.6-luna".to_owned()).with_model_catalog(catalog)
}

fn write_credentials(root: &Path, expires_at_ms: u64) {
    let dir = root.join(".claude");
    std::fs::create_dir_all(&dir).expect("credentials dir");
    std::fs::write(
        dir.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"redacted","refreshToken":"redacted","expiresAt":{expires_at_ms}}}}}"#
        ),
    )
    .expect("write credentials");
}

fn millis(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .expect("unix millis")
        .as_millis() as u64
}

#[test]
fn detects_tui_subscription_oauth_expiry() {
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(TUI_OAUTH)));
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        TUI_OAUTH_RESUME
    )));
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "Claude subscription model claude-opus-5 failed [authentication; exit status: 1]: \
Failed to authenticate: OAuth session expired and could not be refreshed"
    )));
    assert!(!is_subscription_auth_failure(&anyhow::anyhow!(
        "Configured ACP completed with no assistant content"
    )));
    assert!(
        !is_subscription_auth_failure(&anyhow::anyhow!(PROVIDER_401)),
        "Sakana/provider 401 must not be treated as Claude subscription OAuth expiry"
    );
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "please run /login before retrying the subscription model"
    )));
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "provider oauth token expired; refresh required"
    )));
}

#[test]
fn malformed_or_tokenless_credentials_file_is_unknown_not_expired() {
    let root = tempfile::tempdir().expect("malformed fixture");
    let missing_expiry = root.path().join("no-expiry.json");
    std::fs::write(
        &missing_expiry,
        r#"{"claudeAiOauth":{"accessToken":"redacted","refreshToken":"redacted"}}"#,
    )
    .expect("write credentials without expiresAt");
    let malformed = root.path().join("malformed.json");
    std::fs::write(&malformed, "not-json").expect("write malformed credentials");
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert_eq!(credentials_access_expired_at(&missing_expiry, now), None);
    assert_eq!(credentials_access_expired_at(&malformed, now), None);
    assert_eq!(
        credentials_access_expired_at(&root.path().join("missing.json"), now),
        None
    );
    let negative = root.path().join("negative.json");
    std::fs::write(
        &negative,
        r#"{"claudeAiOauth":{"accessToken":"redacted","refreshToken":"redacted","expiresAt":-1}}"#,
    )
    .expect("write negative expiry");
    assert_eq!(credentials_access_expired_at(&negative, now), None);
    let negative_float = root.path().join("negative-float.json");
    std::fs::write(
        &negative_float,
        r#"{"claudeAiOauth":{"accessToken":"redacted","refreshToken":"redacted","expiresAt":-1.5}}"#,
    )
    .expect("write negative float expiry");
    assert_eq!(credentials_access_expired_at(&negative_float, now), None);
    let infinite = root.path().join("infinite.json");
    std::fs::write(
        &infinite,
        r#"{"claudeAiOauth":{"accessToken":"redacted","refreshToken":"redacted","expiresAt":1e309}}"#,
    )
    .expect("write infinite expiry");
    assert_eq!(credentials_access_expired_at(&infinite, now), None);
}

#[test]
fn credentials_file_expiry_is_detected_without_reading_tokens() {
    let root = tempfile::tempdir().expect("oauth fixture");
    let path = root.path().join(".credentials.json");
    let past = millis(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    std::fs::write(
        &path,
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"redacted","refreshToken":"redacted","expiresAt":{past}}}}}"#
        ),
    )
    .expect("write expired credentials");
    assert_eq!(
        credentials_access_expired_at(&path, UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        Some(true)
    );
    assert_eq!(
        credentials_access_expired_at(&path, UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
        Some(false)
    );
}

#[test]
fn subscription_auth_failover_skips_native_claude_and_picks_a_provider() {
    let failover = opus_and_luna_bridge()
        .subscription_auth_failover_for()
        .expect("provider recovery");
    assert_eq!(failover.model, "gpt-5.6-luna");
    assert_eq!(failover.route, RouteDecision::Provider);
    assert_eq!(failover.effort.as_deref(), Some("max"));
}

#[test]
fn expired_credentials_preflight_rewrites_opus_onto_luna() {
    let root = tempfile::tempdir().expect("preflight fixture");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());

    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("medium".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "gpt-5.6-luna");
    assert_eq!(effort.as_deref(), Some("max"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn oauth_cooldown_without_credentials_file_rewrites_onto_provider() {
    let root = tempfile::tempdir().expect("cooldown fixture");
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_OAUTH), Some("claude-opus-5"));
    assert!(
        crate::anthropic::provider_auth_cooldown::scope_is_cooling_down_at(
            bridge.provider_auth_cache_path().as_deref(),
            SUBSCRIPTION_AUTH_SCOPE,
            SystemTime::now(),
        )
    );

    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("medium".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "gpt-5.6-luna");
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn fresh_credentials_keep_subscription_despite_oauth_cooldown() {
    let root = tempfile::tempdir().expect("fresh fixture");
    write_credentials(
        root.path(),
        millis(SystemTime::now() + Duration::from_secs(8 * 60 * 60)),
    );
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_OAUTH), Some("claude-opus-5"));

    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("medium".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "claude-opus-5");
    assert_eq!(effort.as_deref(), Some("medium"));
    assert_eq!(route, RouteDecision::Subscription);
}

#[test]
fn provider_route_is_not_rewritten_by_subscription_oauth_preflight() {
    let bridge = opus_and_luna_bridge();
    let mut request = dummy_request("gpt-5.6-luna");
    let mut effort = Some("max".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
    );
    assert_eq!(request.model, "gpt-5.6-luna");
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn expired_oauth_without_provider_backend_keeps_subscription() {
    let root = tempfile::tempdir().expect("no-provider fixture");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    let bridge =
        Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "claude-opus-5".to_owned())
            .with_model_catalog(catalog)
            .with_usage_limit_cache_home(root.path());
    assert!(bridge.subscription_oauth_is_unusable());
    assert!(bridge.subscription_auth_failover_for().is_none());

    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("medium".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "claude-opus-5");
    assert_eq!(effort.as_deref(), Some("medium"));
    assert_eq!(route, RouteDecision::Subscription);
}

#[test]
fn subscription_auth_failover_skips_exhausted_luna_and_picks_cursor() {
    let root = tempfile::tempdir().expect("exhausted luna fixture");
    let bridge = opus_luna_and_cursor_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(PROVIDER_401), Some("gpt-5.6-luna"));
    assert!(bridge.subagent_provider_is_exhausted("gpt-5.6-luna"));
    assert!(!bridge.subagent_provider_is_exhausted("auto"));

    let failover = bridge
        .subscription_auth_failover_for()
        .expect("cursor recovery");
    assert_eq!(failover.model, "auto");
    assert_eq!(failover.route, RouteDecision::Provider);
    assert_eq!(failover.effort.as_deref(), Some("high"));
}

#[test]
fn exhausted_provider_then_expired_oauth_returns_to_a_sibling_provider() {
    let root = tempfile::tempdir().expect("usage then oauth fixture");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let bridge = fugu_luna_bridge().with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(&anyhow::anyhow!(PROVIDER_401), Some("fugu"));

    let mut request = dummy_request("fugu");
    let mut effort = Some("high".to_owned());
    let route = bridge.apply_usage_limit_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
        false,
    );
    assert_eq!(request.model, "claude-sonnet-5");
    assert_eq!(route, RouteDecision::Subscription);

    let route = bridge.apply_subscription_auth_preflight(&mut request, route, &mut effort);
    assert_eq!(
        request.model, "gpt-5.6-luna",
        "OAuth-dead subscription must not stay on sonnet; skip exhausted fugu"
    );
    assert_eq!(effort.as_deref(), Some("max"));
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn subscription_oauth_usability_follows_credentials_then_cooldown() {
    let root = tempfile::tempdir().expect("usability fixture");
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());
    assert!(
        !bridge.subscription_oauth_is_unusable(),
        "missing credentials and no cooldown must still attempt subscription"
    );

    bridge.note_provider_exhaustion(&anyhow::anyhow!(TUI_OAUTH), Some("claude-opus-5"));
    assert!(bridge.subscription_oauth_is_unusable());

    write_credentials(
        root.path(),
        millis(SystemTime::now() + Duration::from_secs(8 * 60 * 60)),
    );
    assert!(
        !bridge.subscription_oauth_is_unusable(),
        "fresh credentials after /login must clear the unusable state"
    );
}

#[test]
fn is_subscription_auth_failure_true_oauth_expired() {
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "oauth session expired and could not be refreshed"
    )));
}

#[test]
fn is_subscription_auth_failure_true_login_prompt() {
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "please run /login"
    )));
}

#[test]
fn is_subscription_auth_failure_true_oauth_and_expired() {
    assert!(is_subscription_auth_failure(&anyhow::anyhow!(
        "provider oauth token expired; refresh required"
    )));
}

#[test]
fn is_subscription_auth_failure_false_unrelated_401() {
    assert!(!is_subscription_auth_failure(&anyhow::anyhow!(
        PROVIDER_401
    )));
}

#[test]
fn credentials_access_expired_at_with_u64_past_expiry() {
    let root = tempfile::tempdir().expect("u64 fixture");
    let path = root.path().join(".credentials.json");
    let past_ms = millis(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    std::fs::write(
        &path,
        format!(r#"{{"claudeAiOauth":{{"expiresAt":{past_ms}}}}}"#),
    )
    .expect("write u64 expiry");
    assert_eq!(
        credentials_access_expired_at(&path, UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        Some(true)
    );
}

#[test]
fn credentials_access_expired_at_with_future_expiry() {
    let root = tempfile::tempdir().expect("future fixture");
    let path = root.path().join(".credentials.json");
    let future_ms = millis(SystemTime::now() + Duration::from_secs(24 * 60 * 60));
    std::fs::write(
        &path,
        format!(r#"{{"claudeAiOauth":{{"expiresAt":{future_ms}}}}}"#),
    )
    .expect("write future credentials");
    assert_eq!(
        credentials_access_expired_at(&path, SystemTime::now()),
        Some(false)
    );
}

#[test]
fn subscription_oauth_is_unusable_with_expired_credentials() {
    let root = tempfile::tempdir().expect("unusable expired fixture");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());
    assert!(bridge.subscription_oauth_is_unusable());
}

#[test]
fn subscription_oauth_is_unusable_with_valid_future_credentials() {
    let root = tempfile::tempdir().expect("unusable valid fixture");
    write_credentials(
        root.path(),
        millis(SystemTime::now() + Duration::from_secs(24 * 60 * 60)),
    );
    let bridge = opus_and_luna_bridge().with_usage_limit_cache_home(root.path());
    assert!(!bridge.subscription_oauth_is_unusable());
}

#[test]
fn apply_subscription_auth_preflight_non_subscription_route_no_rewrite() {
    let bridge = opus_and_luna_bridge();
    let mut request = dummy_request("gpt-5.6-luna");
    let mut effort = Some("max".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Provider,
        &mut effort,
    );
    assert_eq!(request.model, "gpt-5.6-luna");
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn apply_subscription_auth_preflight_subscription_unusable_no_failover() {
    let root = tempfile::tempdir().expect("no failover fixture");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let mut catalog = ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("install subscription fallback");
    let bridge =
        Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "claude-opus-5".to_owned())
            .with_model_catalog(catalog)
            .with_usage_limit_cache_home(root.path());

    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("medium".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "claude-opus-5");
    assert_eq!(route, RouteDecision::Subscription);
}

#[test]
fn subscription_auth_failover_without_worker_effort_still_rewrites() {
    let root = tempfile::tempdir().expect("oauth failover without effort");
    write_credentials(root.path(), millis(UNIX_EPOCH + Duration::from_secs(1)));
    let backend =
        AgentBackend::spawn_routes(&[BackendRoute::new("auto", BackendKind::ConfiguredAcp)]);
    let bridge = Bridge::new_with_backend(backend, "claude-opus-5".to_owned())
        .with_usage_limit_cache_home(root.path());
    let mut request = dummy_request("claude-opus-5");
    let mut effort = Some("high".to_owned());
    let route = bridge.apply_subscription_auth_preflight(
        &mut request,
        RouteDecision::Subscription,
        &mut effort,
    );
    assert_eq!(request.model, "auto");
    assert_eq!(route, RouteDecision::Provider);
}

#[test]
fn nan_expires_at_is_unknown_not_expired() {
    let root = tempfile::tempdir().expect("nan expiry fixture");
    let path = root.path().join("nan.json");
    std::fs::write(
        &path,
        r#"{"claudeAiOauth":{"accessToken":"x","refreshToken":"y","expiresAt":null}}"#,
    )
    .expect("write null expiry");
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert_eq!(credentials_access_expired_at(&path, now), None);
    let nan = root.path().join("nan-number.json");
    std::fs::write(
        &nan,
        r#"{"claudeAiOauth":{"accessToken":"x","refreshToken":"y","expiresAt":"NaN"}}"#,
    )
    .expect("write nan expiry");
    assert_eq!(credentials_access_expired_at(&nan, now), None);
}

#[test]
fn subscription_auth_failover_skips_models_without_backend() {
    let backend = AgentBackend::spawn_routes(&[]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-gpt",
            "gpt-5.6-luna",
            "max",
        )])
        .expect("install luna worker");
    let bridge =
        Bridge::new_with_backend(backend, "gpt-5.6-luna".to_owned()).with_model_catalog(catalog);
    assert!(bridge.subscription_auth_failover_for().is_none());
}

fn oauth_failing_subscription_program(root: &Path) -> std::path::PathBuf {
    let program = root.join("claude-oauth-fail");
    std::fs::write(
        &program,
        "#!/bin/sh\nprintf '%s\\n' 'oauth session expired and could not be refreshed' >&2\nexit 1\n",
    )
    .expect("write oauth-fail subscription");
    let mut permissions = std::fs::metadata(&program)
        .expect("oauth-fail metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("oauth-fail executable");
    program
}

fn unrelated_failing_subscription_program(root: &Path) -> std::path::PathBuf {
    let program = root.join("claude-boom");
    std::fs::write(&program, "#!/bin/sh\nprintf '%s\\n' 'boom' >&2\nexit 1\n")
        .expect("write boom subscription");
    let mut permissions = std::fs::metadata(&program)
        .expect("boom metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("boom executable");
    program
}

async fn subscription_bridge(
    root: &Path,
    program: std::path::PathBuf,
    exhaust_luna: bool,
) -> Arc<Bridge> {
    let source = root.join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let app_server = root.join("app-server");
    std::fs::write(
        &app_server,
        "#!/bin/sh\nwhile IFS= read -r line; do id=$(printf '%s\\n' \"$line\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p'); printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"; done\n",
    )
    .expect("app-server fixture");
    let mut permissions = std::fs::metadata(&app_server)
        .expect("app-server metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&app_server, permissions).expect("app-server executable");
    let app =
        AppServer::spawn_with_program("gpt-5.6-luna", &app_server, &source, &root.join("isolated"))
            .await
            .expect("start app-server fixture");
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-gpt",
            "gpt-5.6-luna",
            "max",
        )])
        .expect("install luna worker");
    let bridge = Bridge::new_with_subscription_program(app, "gpt-5.6-luna".to_owned(), program)
        .with_model_catalog(catalog);
    if exhaust_luna {
        bridge.note_provider_exhaustion(&anyhow::anyhow!(PROVIDER_401), Some("gpt-5.6-luna"));
    }
    Arc::new(bridge)
}

#[tokio::test]
async fn streaming_subscription_auth_failover_returns_the_stream_without_retry() {
    let bridge = Arc::new(opus_and_luna_bridge());
    let request = dummy_request("claude-opus-5");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bridge.subscription_messages_with_auth_failover(
            request,
            Some("medium".to_owned()),
            false,
            false,
        ),
    )
    .await
    .expect("streaming subscription should return immediately")
    .expect("streaming response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn non_stream_subscription_auth_failure_without_failover_keeps_the_error() {
    let root = tempfile::tempdir().expect("oauth no-failover fixture");
    let bridge = subscription_bridge(
        root.path(),
        oauth_failing_subscription_program(root.path()),
        true,
    )
    .await;
    assert!(bridge.subscription_auth_failover_for().is_none());
    let mut request = dummy_request("claude-opus-5");
    request.stream = false;
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        bridge.subscription_messages_with_auth_failover(
            request,
            Some("medium".to_owned()),
            false,
            false,
        ),
    )
    .await
    .expect("oauth failure should finish")
    .expect_err("oauth failure without failover");
    assert!(is_subscription_auth_failure(&error), "{error:#}");
}

#[tokio::test]
async fn non_stream_subscription_auth_failure_failsover_to_a_provider() {
    let root = tempfile::tempdir().expect("oauth failover fixture");
    let bridge = subscription_bridge(
        root.path(),
        oauth_failing_subscription_program(root.path()),
        false,
    )
    .await;
    assert!(bridge.subscription_auth_failover_for().is_some());
    let mut request = dummy_request("claude-opus-5");
    request.stream = false;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        bridge.subscription_messages_with_auth_failover(
            request,
            Some("medium".to_owned()),
            false,
            false,
        ),
    )
    .await
    .expect("oauth failover should finish");
    match outcome {
        Ok(response) => assert_eq!(response.status(), axum::http::StatusCode::OK),
        Err(error) => assert!(
            !is_subscription_auth_failure(&error),
            "provider failover must leave the subscription OAuth error behind: {error:#}"
        ),
    }
}

#[tokio::test]
async fn non_stream_unrelated_subscription_failure_does_not_failover() {
    let root = tempfile::tempdir().expect("unrelated subscription fixture");
    let bridge = subscription_bridge(
        root.path(),
        unrelated_failing_subscription_program(root.path()),
        false,
    )
    .await;
    let mut request = dummy_request("claude-opus-5");
    request.stream = false;
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        bridge.subscription_messages_with_auth_failover(
            request,
            Some("medium".to_owned()),
            false,
            false,
        ),
    )
    .await
    .expect("unrelated failure should finish")
    .expect_err("unrelated subscription failure");
    assert!(!is_subscription_auth_failure(&error), "{error:#}");
}
