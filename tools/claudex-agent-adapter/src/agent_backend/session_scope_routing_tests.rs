use std::sync::Arc;

use super::*;
use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};

#[test]
fn unguarded_scope_prefers_the_sole_named_claude_session_pool() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let named = scopes.scope(Some("tui-only"));
    let _ = scopes.scope(None);
    assert!(Arc::ptr_eq(&named, &scopes.unguarded_scope()));
}

#[test]
fn unguarded_scope_falls_back_to_anonymous_when_multiple_named_pools_exist() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("tui-a"));
    let _ = scopes.scope(Some("tui-b"));
    let anonymous = scopes.scope(None);
    assert!(Arc::ptr_eq(&anonymous, &scopes.unguarded_scope()));
}

#[tokio::test]
async fn top_level_respond_finds_unique_started_pool_for_provider_models() {
    for model in [
        "glm-5.2:cloud",
        "gpt-5.4",
        "grok-4-latest",
        "auto",
        "fugu",
        "muse-spark",
    ] {
        assert_respond_finds_unique_started_pool(model).await;
    }
}

/// A single Claude-session pool that started `model` must be the one the
/// top-level `respond_for_model` targets, even with an unguarded anonymous scope.
async fn assert_respond_finds_unique_started_pool(model: &str) {
    let scoped =
        AgentBackend::spawn_routes(&[BackendRoute::new(model, BackendKind::CodexAppServer)]);
    let AgentBackend::SessionScoped(scopes) = scoped.as_ref() else {
        panic!("expected SessionScoped backends");
    };
    let leaf = Arc::new(AgentBackend::Grok(crate::grok_acp::GrokAcp::alive_for_test()));
    scopes.insert_scope_for_test(
        "tui-session",
        AgentBackend::routed(vec![(model.to_owned(), Arc::clone(&leaf))]),
    );
    let _ = scopes.scope(None);
    let error = scoped
        .respond_for_model(model, serde_json::json!(1), serde_json::json!({}))
        .await
        .expect_err("Grok leaf rejects Claude tool results");
    let message = error.to_string();
    assert!(
        !message.contains("not initialized"),
        "{model}: tool result must not hit the uninitialized anonymous pool: {message}"
    );
    assert!(
        message.contains("Grok ACP"),
        "{model}: expected the started Claude-session pool: {message}"
    );
}

#[tokio::test]
async fn top_level_respond_does_not_guess_when_two_sessions_started_the_same_model() {
    let model = "glm-5.2:cloud";
    let scoped =
        AgentBackend::spawn_routes(&[BackendRoute::new(model, BackendKind::CodexAppServer)]);
    let AgentBackend::SessionScoped(scopes) = scoped.as_ref() else {
        panic!("expected SessionScoped backends");
    };
    for id in ["tui-a", "tui-b"] {
        let leaf = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        scopes.insert_scope_for_test(id, AgentBackend::routed(vec![(model.to_owned(), leaf)]));
    }
    let error = scoped
        .respond_for_model(model, serde_json::json!(1), serde_json::json!({}))
        .await
        .expect_err("ambiguous scopes must not invent a ready backend");
    assert!(
        error.to_string().contains("not initialized"),
        "expected anonymous miss when multiple sessions share the model: {error}"
    );
}
