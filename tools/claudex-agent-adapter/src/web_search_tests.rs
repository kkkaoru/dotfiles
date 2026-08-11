use super::*;

#[test]
fn serializes_each_search_mode_and_keeps_the_default_compact() {
    assert_eq!(WebSearchMode::default().as_str(), "delegate-ccr");
    assert!(WebSearchMode::default().is_default());
    for (mode, name) in [
        (WebSearchMode::CodexNative, "codex-native"),
        (WebSearchMode::AcpNative, "acp-native"),
        (WebSearchMode::DelegateMcp, "delegate-mcp"),
        (WebSearchMode::Disabled, "disabled"),
    ] {
        assert_eq!(mode.as_str(), name);
        assert!(!mode.is_default());
        assert_eq!(mode.to_string(), name);
        let encoded = serde_json::to_value(mode).expect("search mode JSON");
        assert_eq!(encoded, name);
        assert_eq!(
            serde_json::from_value::<WebSearchMode>(encoded).unwrap(),
            mode
        );
    }
    assert!(WebSearchMode::AcpNative.uses_provider_native_agent_loop());
    assert!(WebSearchMode::DelegateMcp.uses_provider_native_agent_loop());
    assert!(!WebSearchMode::CodexNative.uses_provider_native_agent_loop());
    assert!(!WebSearchMode::DelegateCcr.uses_provider_native_agent_loop());
}

#[test]
fn parses_and_deduplicates_provider_results() {
    let event = serde_json::json!({
        "params": {"item": {"type": "webSearch", "results": [
            {"title":"First", "url":"https://example.test/a", "snippet":"one"},
            {"title":"duplicate", "url":"https://example.test/a"},
            {"title":"missing-url"}, {"title":"", "url":"https://example.test/b"}
        ]}}
    });
    let mut results = Vec::new();
    assert!(is_web_search(&event));
    collect_item_results(&event, &mut results);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "First");
    assert_eq!(results[0].snippet.as_deref(), Some("one"));
    assert!(!is_web_search(
        &serde_json::json!({"params":{"item":{"type":"text"}}})
    ));
}

#[test]
fn extracts_http_urls_and_rejects_empty_queries_without_workers() {
    let results = extract_urls("See (https://example.test/a), http://example.test/b.");
    assert_eq!(
        results
            .iter()
            .map(|result| result.url.as_str())
            .collect::<Vec<_>>(),
        ["https://example.test/a", "http://example.test/b"]
    );
    assert!(extract_urls("no links here").is_empty());
}

#[test]
fn does_not_treat_prose_urls_as_a_live_search_result_without_an_event() {
    assert!(fallback_results(0, "https://example.test/from-memory").is_empty());
    assert_eq!(
        fallback_results(1, "https://example.test/after-search")[0].url,
        "https://example.test/after-search"
    );
}

#[test]
fn rejects_malformed_results_and_answer_deltas() {
    let mut answer = String::new();
    append_answer_delta(&serde_json::json!({"params": {"delta": "ok"}}), &mut answer);
    append_answer_delta(&serde_json::json!({"params": {"delta": 1}}), &mut answer);
    append_answer_delta(&serde_json::json!({"params": {}}), &mut answer);
    assert_eq!(answer, "ok");

    let mut results = Vec::new();
    collect_item_results(
        &serde_json::json!({"params": {"item": {"type": "webSearch"}}}),
        &mut results,
    );
    assert!(results.is_empty());
    assert!(parse_result(&serde_json::json!({"title": " ", "url": "https://x"})).is_none());
    assert!(parse_result(&serde_json::json!({"title": "x", "url": " "})).is_none());
}

#[tokio::test]
async fn rejects_empty_queries_and_missing_workers() {
    let backend = AgentBackend::routed(Vec::new());
    let empty = run(&backend, &[], " ").await.expect_err("empty query");
    assert!(empty.to_string().contains("must not be empty"));
    let no_workers = run(&backend, &[], "query").await.expect_err("no workers");
    assert!(no_workers.to_string().contains("no WebSearch worker"));
    let worker = WorkerRoute::new("worker".to_owned(), "model".to_owned(), "high".to_owned());
    let failed = run(&backend, std::slice::from_ref(&worker), "query")
        .await
        .expect_err("unrouted search worker");
    assert!(failed.to_string().contains("all WebSearch workers failed"));
}
