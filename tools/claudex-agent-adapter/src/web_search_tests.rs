use super::*;
use crate::app_server::events::ThreadEventDispatcher;

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

#[tokio::test]
async fn injected_worker_events_cover_native_results_and_prose_fallback() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("native-search");
    dispatcher.dispatch(serde_json::json!({
        "method":"item/started",
        "params":{"threadId":"native-search", "item":{
            "type":"webSearch",
            "results":[{"title":"Native", "url":"https://example.test/native"}]
        }}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"item/completed",
        "params":{"threadId":"native-search", "item":{
            "type":"webSearch",
            "results":[{"title":"Duplicate", "url":"https://example.test/native"}]
        }}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"native-search", "delta":"ignored prose"}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"turn/completed", "params":{"threadId":"native-search"}
    }));
    let native = run_worker_with_events("query", std::future::ready(Ok(events)))
        .await
        .expect("native search result");
    assert_eq!(native.query, "query");
    assert_eq!(native.search_count, 1);
    assert_eq!(native.results.len(), 1);
    assert_eq!(native.results[0].title, "Native");

    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("fallback-search");
    dispatcher.dispatch(serde_json::json!({
        "method":"item/started",
        "params":{"threadId":"fallback-search", "item":{"type":"webSearch"}}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"fallback-search", "delta":"https://example.test/fallback"}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"error", "params":{"threadId":"fallback-search"}
    }));
    let fallback = run_worker_with_events("fallback", std::future::ready(Ok(events)))
        .await
        .expect("fallback search result");
    assert_eq!(fallback.search_count, 1);
    assert_eq!(fallback.results[0].url, "https://example.test/fallback");
}

#[tokio::test]
async fn empty_and_unknown_worker_events_finish_without_results() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("empty-search");
    dispatcher.dispatch(serde_json::json!({
        "method": "item/started",
        "params": {"threadId": "empty-search", "item": {"type": "text"}}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method": "unknown",
        "params": {"threadId": "empty-search"}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method": "turn/completed", "params": {"threadId": "empty-search"}
    }));
    let response = run_worker_with_events("empty", std::future::ready(Ok(events)))
        .await
        .expect("empty worker response");
    assert!(response.results.is_empty());
    assert_eq!(response.search_count, 0);
}

#[tokio::test]
async fn injected_worker_start_and_timeout_failures_are_deterministic() {
    let start_error = run_worker_with_events(
        "query",
        std::future::ready(Err(anyhow::anyhow!("injected start failure"))),
    )
    .await
    .expect_err("injected start failure");
    assert!(start_error.to_string().contains("injected start failure"));

    let worker = WorkerRoute::new("worker".to_owned(), "model".to_owned(), "high".to_owned());
    let response = SearchResponse {
        query: "query".to_owned(),
        results: Vec::new(),
        search_count: 0,
    };
    assert_eq!(
        wait_for_worker(
            &worker,
            Duration::from_secs(1),
            std::future::ready(Ok(response.clone())),
        )
        .await
        .expect("ready worker"),
        response
    );
    let propagated = wait_for_worker(
        &worker,
        Duration::from_secs(1),
        std::future::ready(Err(anyhow::anyhow!("worker failed"))),
    )
    .await
    .expect_err("worker failure");
    assert!(propagated.to_string().contains("worker failed"));
    let timed_out = wait_for_worker(
        &worker,
        Duration::ZERO,
        std::future::pending::<Result<SearchResponse>>(),
    )
    .await
    .expect_err("pending worker timeout");
    assert!(timed_out.to_string().contains("model timed out"));
}

#[tokio::test]
async fn collect_worker_response_covers_closed_channel_and_non_search_items() {
    let closed = crate::app_server::events::ThreadEvents::closed("closed-search");
    let empty = run_worker_with_events("closed", std::future::ready(Ok(closed)))
        .await
        .expect("closed channel");
    assert_eq!(empty.search_count, 0);
    assert!(empty.results.is_empty());

    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("non-search");
    dispatcher.dispatch(serde_json::json!({
        "method":"item/started",
        "params":{"threadId":"non-search", "item":{"type":"agentMessage"}}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"item/completed",
        "params":{"threadId":"non-search", "item":{"type":"agentMessage"}}
    }));
    dispatcher.dispatch(serde_json::json!({
        "method":"turn/completed",
        "params":{"threadId":"non-search"}
    }));
    let skipped = run_worker_with_events("non-search", std::future::ready(Ok(events)))
        .await
        .expect("non-search items");
    assert_eq!(skipped.search_count, 0);
}

#[test]
fn t4_failed_search_errors_are_human_and_hide_transport_codes() {
    assert_eq!(
        human_web_search_error(&anyhow::anyhow!("query must not be empty")),
        "WebSearch query must not be empty"
    );
    assert_eq!(
        human_web_search_error(&anyhow::anyhow!("no WebSearch worker is configured")),
        "No WebSearch worker is configured"
    );
    let timeout = human_web_search_error(&anyhow::anyhow!("worker model timed out"));
    assert_eq!(timeout, "WebSearch timed out");
    for detail in ["worker timeout", "worker ETIMEDOUT"] {
        assert_eq!(
            human_web_search_error(&anyhow::anyhow!(detail)),
            "WebSearch timed out"
        );
    }
    let transport = human_web_search_error(&anyhow::anyhow!(
        "proxy PROXY_TRANSPORT failed with ECONNABORTED ETIMEDOUT ECONNRESET EAI_AGAIN socket hang up"
    ));
    assert_eq!(transport, "WebSearch timed out");
    for detail in [
        "ECONNRESET from upstream",
        "getaddrinfo EAI_AGAIN search.example",
        "socket hang up",
        "PROXY_TRANSPORT ECONNABORTED",
    ] {
        let unknown = human_web_search_error(&anyhow::anyhow!("{detail}"));
        assert_eq!(unknown, "WebSearch failed", "{detail}");
    }
    let unknown = human_web_search_error(&anyhow::anyhow!(
        "proxy PROXY_TRANSPORT failed with ECONNABORTED ECONNRESET EAI_AGAIN socket hang up"
    ));
    assert_eq!(unknown, "WebSearch failed");
    assert!(!unknown.contains("PROXY_TRANSPORT"), "{unknown}");
    assert!(!unknown.contains("ECONNABORTED"), "{unknown}");
    assert!(!unknown.contains("ECONNRESET"), "{unknown}");
    assert!(!unknown.contains("EAI_AGAIN"), "{unknown}");
}

#[tokio::test]
async fn parallel_search_covers_empty_failed_and_panicking_workers() {
    let none = Vec::<std::future::Ready<Result<SearchResponse>>>::new();
    let missing = first_nonempty_response("q", none)
        .await
        .expect_err("no workers");
    assert!(missing.to_string().contains("no WebSearch worker"));

    type Job = std::pin::Pin<Box<dyn std::future::Future<Output = Result<SearchResponse>> + Send>>;
    let jobs: Vec<Job> = vec![
        Box::pin(std::future::ready(Ok(SearchResponse {
            query: "q".to_owned(),
            results: Vec::new(),
            search_count: 0,
        }))),
        Box::pin(std::future::ready(Err(anyhow::anyhow!("worker failed")))),
        Box::pin(async { panic!("worker panicked") }),
    ];
    let failed = first_nonempty_response("q", jobs)
        .await
        .expect_err("all workers fail");
    let message = failed.to_string();
    assert!(message.contains("returned no results"), "{message}");
    assert!(message.contains("worker failed"), "{message}");
    assert!(message.contains("join failed"), "{message}");
}

#[tokio::test]
async fn t13_parallel_workers_let_the_first_nonempty_win() {
    let slow_empty = async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(SearchResponse {
            query: "q".to_owned(),
            results: Vec::new(),
            search_count: 0,
        })
    };
    let fast = async {
        Ok(SearchResponse {
            query: "q".to_owned(),
            results: vec![SearchResult {
                title: "hit".to_owned(),
                url: "https://example.test/fast".to_owned(),
                snippet: None,
            }],
            search_count: 1,
        })
    };
    let started = std::time::Instant::now();
    let jobs = [
        Box::pin(slow_empty)
            as std::pin::Pin<Box<dyn std::future::Future<Output = Result<SearchResponse>> + Send>>,
        Box::pin(fast),
    ];
    let won = first_nonempty_response("q", jobs)
        .await
        .expect("fast worker");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "serial search would wait on the empty worker"
    );
    assert_eq!(won.results[0].title, "hit");
}
