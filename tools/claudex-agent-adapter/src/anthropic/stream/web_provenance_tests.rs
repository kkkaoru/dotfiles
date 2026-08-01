#[test]
fn detects_only_explicit_user_web_requests_or_retrieval_workers() {
    assert!(requires_verified_web_evidence(
        &[json!({"role":"user","content":"WebSearch で AVITA を調査して"})],
        &json!("ordinary bridge instructions"),
    ));
    assert!(requires_verified_web_evidence(
        &[json!({"role":"user","content":"summarize this"})],
        &json!("name: claudex-haiku-search\ntools: WebSearch,WebFetch"),
    ));
    assert!(!requires_verified_web_evidence(
        &[json!({"role":"user","content":"summarize this"})],
        &json!("The bridge supports tools such as WebSearch."),
    ));
    assert!(is_dedicated_live_web_worker(&json!("Dedicated live-web retrieval worker")));
    assert!(is_dedicated_live_web_worker(&json!("tools: WebSearch,WebFetch")));
    assert!(!is_dedicated_live_web_worker(&json!("ordinary worker")));
    assert!(explicitly_requests_live_web("Please SEARCH THE WEB for current facts."));
}

#[test]
fn accepts_only_successful_native_search_completion_with_an_id() {
    let event = json!({
        "method":"item/completed",
        "params":{"item":{"type":"webSearch","id":"search-1","results":[{"url":"https://example.test/avita"}]}}
    });
    assert!(matches!(
        native_web_event(&event),
        Some(NativeWebEvent::Completed { call_id: "search-1" })
    ));
    for item in [
        json!({"type":"webSearch","id":"failed","status":"failed"}),
        json!({"type":"webSearch","id":"error","error":"network"}),
        json!({"type":"webSearch","id":""}),
        json!({"type":"webSearch","id":"none","results":null}),
        json!({"type":"webSearch","id":"empty","results":[]}),
        json!({"type":"webSearch","id":"text","results":"https://example.test"}),
        json!({"type":"message","id":"message-1"}),
    ] {
        let event = json!({"method":"item/completed","params":{"item":item}});
        assert!(native_web_event(&event).is_none());
    }
}

#[test]
fn retains_started_status_without_treating_it_as_evidence() {
    let event = json!({
        "method":"item/started",
        "params":{"item":{"type":"webSearch","query":"AVITA"}}
    });
    assert!(matches!(
        native_web_event(&event),
        Some(NativeWebEvent::Started { query: "AVITA" })
    ));
}

#[tokio::test]
async fn gates_unverified_web_answer_but_preserves_verified_and_tool_turns() {
    let mut unverified = SegmentBuilder::new(3);
    unverified.requires_verified_web_evidence = true;
    unverified.usage.output_tokens = 999;
    unverified.blocks = vec![json!({"type":"text","text":"https://invented.example"})];
    let unverified = unverified.finish(None).await.expect("unverified segment");
    assert_eq!(
        unverified.blocks,
        [json!({"type":"text","text":UNVERIFIED_WEB_RESPONSE})]
    );
    assert!(unverified.usage.output_tokens > 0);
    assert_ne!(unverified.usage.output_tokens, 999);

    let mut verified = SegmentBuilder::new(3);
    verified.requires_verified_web_evidence = true;
    verified.blocks = vec![json!({"type":"text","text":"verified source"})];
    assert!(verified.mark_verified_web_evidence("search-1"));
    let verified = verified.finish(None).await.expect("verified segment");
    assert_eq!(verified.blocks[0]["text"], "verified source");

    let mut tool_turn = SegmentBuilder::new(3);
    tool_turn.requires_verified_web_evidence = true;
    tool_turn.blocks = vec![json!({"type":"text","text":"needs WebSearch result"})];
    tool_turn.gate_unverified_web_response("tool_use");
    assert_eq!(tool_turn.blocks[0]["text"], "needs WebSearch result");
}

#[tokio::test]
async fn counts_only_native_completion_with_structured_retrieval_results() {
    let mut builder = SegmentBuilder::new(1);
    let started = json!({
        "method":"item/started",
        "params":{"item":{"type":"webSearch","id":"search-1","query":"AVITA"}}
    });
    let empty_completion = json!({
        "method":"item/completed",
        "params":{"item":{"type":"webSearch","id":"search-1","results":[]}}
    });
    let completed = json!({
        "method":"item/completed",
        "params":{"item":{"type":"webSearch","id":"search-1","results":[{"url":"https://example.test"}]}}
    });
    builder
        .native_web_search_event(&started, None)
        .await
        .expect("started event");
    builder
        .native_web_search_event(&empty_completion, None)
        .await
        .expect("empty completion");
    assert_eq!(builder.usage.web_search_requests, 0);
    assert!(!builder.has_verified_web_evidence());
    builder
        .native_web_search_event(&completed, None)
        .await
        .expect("completed event");
    builder
        .native_web_search_event(&completed, None)
        .await
        .expect("duplicate event");
    assert_eq!(builder.usage.web_search_requests, 1);
    assert!(builder.has_verified_web_evidence());
}
