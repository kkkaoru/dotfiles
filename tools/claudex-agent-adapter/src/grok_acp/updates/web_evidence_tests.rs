fn search(query: &str) -> WebOperation {
    WebOperation {
        kind: "web_search",
        query: Some(query.into()),
        url: None,
    }
}

struct PanicWhileLocked;

impl Drop for PanicWhileLocked {
    fn drop(&mut self) {
        panic!("poison tracker");
    }
}

#[test]
fn completion_is_correlated_deduplicated_and_session_scoped() {
    let evidence = ProviderWebEvidence::default();
    evidence.record("one", "call", search("one"));
    assert_eq!(evidence.complete("two", "call", None), None);
    assert_eq!(evidence.complete("one", "call", None), Some(search("one")));
    assert_eq!(evidence.complete("one", "call", None), None);
    evidence.clear("one");
    assert_eq!(evidence.complete("one", "call", None), None);
}

#[test]
fn completed_update_can_supply_the_explicit_operation() {
    let evidence = ProviderWebEvidence::default();
    assert_eq!(
        evidence.complete("session", "call", Some(search("query"))),
        Some(search("query"))
    );
    assert_eq!(
        evidence.complete("session", "call", Some(search("query"))),
        None
    );
}

#[test]
fn accepts_only_explicit_web_tools_and_nonempty_provider_results() {
    assert_eq!(
        web_operation(
            "Search workspace",
            Some(acp::ToolKind::Search),
            Some(&json!({"query":"AVITA"}))
        ),
        None
    );
    let search = web_operation(
        "Using WebSearch…",
        Some(acp::ToolKind::Search),
        Some(&json!({"query":"AVITA"})),
    )
    .expect("explicit web search");
    assert!(
        completion_evidence(
            search.clone(),
            Some(json!("provider result https://example.com/search")),
            None
        )
        .is_some()
    );
    assert!(completion_evidence(search.clone(), Some(json!({})), None).is_none());
    assert!(completion_evidence(search, Some(json!("")), None).is_none());

    let fetch = web_operation(
        "Fetch page",
        Some(acp::ToolKind::Fetch),
        Some(&json!({"url":"https://example.com/a"})),
    )
    .expect("http fetch");
    let evidence = completion_evidence(fetch, Some(json!("source https://example.com/b.")), None)
        .expect("provider result");
    assert_eq!(evidence["evidence_class"], "fetch_verified");
    assert_eq!(
        evidence["source_urls"],
        json!(["https://example.com/a", "https://example.com/b"])
    );
}

#[test]
fn recognizes_explicit_variants_and_rejects_ambiguous_fetches() {
    let operation = web_operation(
        "SearchTheWeb: AVITA",
        None,
        Some(&json!({"query":"  AVITA  "})),
    )
    .expect("explicit web search");
    assert_eq!(operation, search("AVITA"));

    let fetch = web_operation(
        "WebFetch: company",
        None,
        Some(&json!({"url":"ftp://example.com"})),
    )
    .expect("explicit web fetch");
    assert_eq!(fetch.kind, "web_fetch");
    assert_eq!(fetch.url, None);

    assert_eq!(
        web_operation(
            "Fetch",
            Some(acp::ToolKind::Fetch),
            Some(&json!({"url":""}))
        ),
        None
    );
    assert_eq!(
        web_operation(
            "Fetch",
            Some(acp::ToolKind::Fetch),
            Some(&json!({"url":"http://example.com/page"})),
        )
        .expect("HTTP fetch")
        .url,
        Some("http://example.com/page".into())
    );
    assert_eq!(
        web_operation("WebSearch", None, Some(&json!("not an object")))
            .expect("search without input")
            .query,
        None
    );
}

#[test]
fn records_limits_and_tolerates_a_poisoned_tracker() {
    let evidence = ProviderWebEvidence::default();
    evidence.record("kept", "call", search("one"));
    evidence.record("kept", "call", search("replacement"));
    assert_eq!(
        evidence.completion_candidate("kept", "call", None),
        Some(search("one"))
    );
    assert!(!evidence.mark_completed("kept", "missing"));
    assert!(evidence.mark_completed("kept", "call"));
    assert!(!evidence.mark_completed("kept", "call"));

    for index in 0..(MAX_TRACKED_CALLS - 1) {
        evidence.record("filled", &index.to_string(), search("filled"));
    }
    assert_eq!(
        evidence.completion_candidate("full", "overflow", Some(search("overflow"))),
        None
    );
    evidence.clear("kept");
    assert_eq!(evidence.completion_candidate("kept", "call", None), None);

    let poisoned = ProviderWebEvidence::default();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let lock = poisoned.calls.lock().expect("lock tracker");
        let panic_while_locked = PanicWhileLocked;
        drop(panic_while_locked);
        drop(lock);
    }));
    assert!(poisoned.calls.lock().is_err());
    poisoned.record("session", "call", search("ignored"));
    assert_eq!(poisoned.completion_candidate("session", "call", None), None);
    assert!(!poisoned.mark_completed("session", "call"));
    poisoned.clear("session");
}

#[test]
fn builds_evidence_from_content_and_covers_output_shapes() {
    let content = vec![
        acp::ContentBlock::Text(acp::TextContent::new("https://example.com/content")).into(),
        acp::ContentBlock::Image(acp::ImageContent::new("data", "image/png")).into(),
        acp::ToolCallContent::Terminal(acp::Terminal::new("terminal")),
    ];
    assert_eq!(
        provider_output(
            Some(json!("same")),
            Some(&vec![
                acp::ContentBlock::Text(acp::TextContent::new("same")).into()
            ])
        ),
        "same"
    );
    assert_eq!(
        provider_output(Some(json!("raw")), Some(&content)),
        "raw\nhttps://example.com/content"
    );
    assert_eq!(
        provider_output(None, Some(&content)),
        "https://example.com/content"
    );
    assert_eq!(provider_output(None, None), "");

    let evidence = completion_evidence(search("AVITA"), None, Some(&content))
        .expect("content URL is evidence");
    assert_eq!(evidence["evidence_class"], "search_result_only");
    assert_eq!(evidence["query"], "AVITA");
    assert!(completion_evidence(search("AVITA"), Some(json!("null")), None).is_none());
    assert!(completion_evidence(search("AVITA"), Some(json!("[]")), None).is_none());
}

#[test]
fn summarizes_and_extracts_only_http_sources() {
    let output = format!(
        "{} https://example.com/path). http://second.example/x,",
        "x ".repeat(321)
    );
    let summary = summary(&output);
    assert!(summary.ends_with('…'));
    assert_eq!(
        extract_source_urls(&output),
        vec![
            "https://example.com/path".to_owned(),
            "http://second.example/x".to_owned(),
        ]
    );
    assert!(extract_source_urls("ftp://example.com javascript://x").is_empty());
    assert!(!is_http_url("https://"));
    assert!(!is_http_url("mailto:research@example.com"));
    assert!(meaningful_provider_output("result"));
    assert!(!meaningful_provider_output("{}"));
}
