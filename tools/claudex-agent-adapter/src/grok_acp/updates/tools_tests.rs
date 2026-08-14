#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::app_server::ThreadEvents;
    use serde_json::json;

    async fn collect_messages(receiver: ThreadEvents) -> Vec<Value> {
        let mut messages = Vec::new();
        while let Ok(Some(message)) =
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv()).await
        {
            messages.push(message);
        }
        messages
    }

    fn dispatch_optional_updates(
        events: &ThreadEventDispatcher,
        updates: impl IntoIterator<Item = acp::ToolCallUpdateFields>,
    ) {
        for (index, fields) in updates.into_iter().enumerate() {
            dispatch_provider_tool_update(
                events,
                "session",
                acp::ToolCallUpdate::new(format!("optional-{index}"), fields),
            );
        }
    }

    fn dispatch_web_completion_updates(
        events: &ThreadEventDispatcher,
        evidence: &ProviderWebEvidence,
    ) {
        for output in ["", "https://example.com/result", "retry output"] {
            dispatch_provider_tool_update_with_evidence(
                events,
                Some(evidence),
                "session",
                acp::ToolCallUpdate::new(
                    "native-search",
                    acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Completed)
                        .raw_output(json!(output)),
                ),
            );
        }
    }

    fn is_only_location(event: &Value) -> bool {
        event["params"]["arguments"]["locations"][0]["path"] == "only-location"
    }

    #[test]
    fn t6_titleless_inprogress_and_statusless_kind_open_wip() {
        let titleless = update_to_tool_call(
            "read-1",
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::InProgress)
                .kind(acp::ToolKind::Read),
        )
        .expect("title-less InProgress");
        assert_eq!(titleless.title, "Read");
        let statusless = update_to_tool_call(
            "read-2",
            acp::ToolCallUpdateFields::new().kind(acp::ToolKind::Read),
        )
        .expect("status-less kind");
        assert_eq!(statusless.title, "Read");
        assert!(
            update_to_tool_call(
                "loc-only",
                acp::ToolCallUpdateFields::new()
                    .locations(vec![acp::ToolCallLocation::new("only")]),
            )
            .is_none(),
            "content-only patches without status/kind stay dropped"
        );
        assert!(
            update_to_tool_call(
                "done",
                acp::ToolCallUpdateFields::new()
                    .title("Read")
                    .status(acp::ToolCallStatus::Completed),
            )
            .is_none(),
            "Completed must stay on providerTool/update"
        );

        let titled = update_to_tool_call(
            "custom-id",
            acp::ToolCallUpdateFields::new()
                .title(" Custom ")
                .status(acp::ToolCallStatus::Pending),
        )
        .expect("explicit title");
        assert_eq!(titled.title, "Custom");

        let call_id = update_to_tool_call(
            "fallback-id",
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Pending),
        )
        .expect("call id fallback");
        assert_eq!(call_id.title, "fallback-id");

        let generic = update_to_tool_call(
            "",
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Pending),
        )
        .expect("generic fallback");
        assert_eq!(generic.title, "provider tool");

        let status_only = acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::InProgress);
        assert!(status_only_params("session", "call", &status_only).is_some());
        let with_output = acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::InProgress)
            .raw_output(json!("output"));
        assert!(status_only_params("session", "call", &with_output).is_none());
        let with_content = acp::ToolCallUpdateFields::new()
            .status(acp::ToolCallStatus::InProgress)
            .content(vec![text("content")]);
        assert!(status_only_params("session", "call", &with_content).is_none());
        let completed = acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed);
        assert!(status_only_params("session", "call", &completed).is_none());
    }

    #[tokio::test]
    async fn t6_dispatches_fallback_updates_as_provider_calls() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        dispatch_fallback_updates(&events);

        let first = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
            .await
            .expect("first fallback update")
            .expect("first dispatched provider call");
        let second = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
            .await
            .expect("second fallback update")
            .expect("second dispatched provider call");
        assert_fallback_update(&first, "titleless-in-progress");
        assert_fallback_update(&second, "statusless-kind");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv())
                .await
                .is_err(),
            "fallback dispatcher emitted more than the two expected calls"
        );
    }

    fn dispatch_fallback_updates(events: &ThreadEventDispatcher) {
        let updates = [
            (
                "titleless-in-progress",
                acp::ToolCallUpdateFields::new()
                    .kind(acp::ToolKind::Read)
                    .status(acp::ToolCallStatus::InProgress),
            ),
            (
                "statusless-kind",
                acp::ToolCallUpdateFields::new().kind(acp::ToolKind::Read),
            ),
        ];
        for (call_id, fields) in updates {
            dispatch_provider_tool_update(
                events,
                "session",
                acp::ToolCallUpdate::new(call_id, fields),
            );
        }
    }

    fn assert_fallback_update(message: &Value, call_id: &str) {
        assert_eq!(message["method"], "item/providerTool/call");
        assert_eq!(message["params"]["callId"], call_id);
        assert_eq!(message["params"]["title"], "Read");
        assert_eq!(message["params"]["tool"], "Read");
        assert_eq!(message["params"]["status"], "in_progress");
    }

    #[test]
    fn labels_every_tool_kind_status_and_title_shape() {
        let kinds = [
            (acp::ToolKind::Read, "Read"),
            (acp::ToolKind::Edit, "Edit"),
            (acp::ToolKind::Execute, "Bash"),
            (acp::ToolKind::Search, "Search"),
            (acp::ToolKind::Fetch, "WebFetch"),
            (acp::ToolKind::Delete, "Delete"),
            (acp::ToolKind::Move, "Move"),
            (acp::ToolKind::Think, "Think"),
            (acp::ToolKind::SwitchMode, "SwitchMode"),
        ];
        for (kind, expected) in kinds {
            assert_eq!(tool_kind_name(kind), Some(expected));
            assert_eq!(tool_kind_label(kind), expected);
        }
        assert_eq!(tool_kind_name(acp::ToolKind::Other), None);
        assert_eq!(tool_kind_label(acp::ToolKind::Other), "other");

        for (status, expected) in [
            (acp::ToolCallStatus::Completed, "completed"),
            (acp::ToolCallStatus::Failed, "failed"),
            (acp::ToolCallStatus::InProgress, "in_progress"),
            (acp::ToolCallStatus::Pending, "pending"),
        ] {
            assert_eq!(tool_status_label(status), expected);
        }

        for (title, expected) in [
            ("Using deploy…", "deploy"),
            ("read_file: target", "read_file"),
            ("two words: target", "two words: target"),
            (": target", ": target"),
            ("", "Tool"),
        ] {
            assert_eq!(
                tool_display_name(&acp::ToolCall::new("id", title)),
                expected
            );
        }
        assert_eq!(
            tool_display_name(
                &acp::ToolCall::new("mcp-call", "MCP").raw_input(json!({"_toolName":"Task"}))
            ),
            "Task"
        );
    }

    #[test]
    fn enriches_every_content_location_and_output_shape() {
        let content = vec![
            text("text"),
            text(""),
            acp::ContentBlock::Image(acp::ImageContent::new("data", "image/png")).into(),
            acp::Diff::new("changed.txt", "new").old_text("old").into(),
            acp::ToolCallContent::Terminal(acp::Terminal::new("terminal-1")),
        ];
        let locations = vec![
            acp::ToolCallLocation::new("one.txt").line(7),
            acp::ToolCallLocation::new("two.txt"),
        ];
        let value = enrich_arguments(json!("raw"), &Some(content.clone()), &Some(locations));
        assert_eq!(value["value"], "raw");
        assert_eq!(value["locations"][0]["line"], 7);
        assert!(
            value["content"]
                .as_str()
                .unwrap()
                .contains("diff changed.txt")
        );
        assert!(
            value["content"]
                .as_str()
                .unwrap()
                .contains("terminal terminal-1")
        );

        assert_eq!(enrich_arguments(Value::Null, &None, &None), json!({}));
        let retained = enrich_arguments(
            json!({"content":"original"}),
            &Some(vec![text("replacement")]),
            &Some(Vec::new()),
        );
        assert_eq!(retained["content"], "original");
        assert_eq!(
            combine_output(Some(json!("raw")), Some(&content)),
            Some(json!(format!("raw\n{}", tool_content_text(&content))))
        );
        assert_eq!(combine_output(Some(json!(7)), None), Some(json!(7)));
        assert_eq!(
            combine_output(None, Some(&vec![text("only")])),
            Some(json!("only"))
        );
        assert_eq!(combine_output(None, None), None);
        assert_eq!(
            combine_output(Some(json!("same")), Some(&vec![text("same")])),
            Some(json!("same"))
        );
        assert_eq!(
            enrich_arguments(json!({}), &Some(Vec::new()), &Some(Vec::new())),
            json!({})
        );
    }

    #[tokio::test]
    async fn dispatches_complete_calls_incremental_updates_and_plan_variants() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let call = acp::ToolCall::new("call", "Edit file")
            .kind(acp::ToolKind::Other)
            .status(acp::ToolCallStatus::Completed)
            .content(vec![text("content")])
            .locations(vec![acp::ToolCallLocation::new(PathBuf::from("file"))]);
        dispatch_provider_tool_call(&events, "session", call);

        let pending = acp::ToolCallUpdateFields::new()
            .title("Write")
            .kind(acp::ToolKind::Edit)
            .status(acp::ToolCallStatus::InProgress)
            .raw_input(json!({"path":"file"}))
            .content(vec![text("body")])
            .locations(vec![acp::ToolCallLocation::new("file").line(2)]);
        dispatch_provider_tool_update(
            &events,
            "session",
            acp::ToolCallUpdate::new("pending", pending),
        );
        dispatch_provider_tool_update(
            &events,
            "session",
            acp::ToolCallUpdate::new(
                "partial",
                acp::ToolCallUpdateFields::new()
                    .title("Partial")
                    .content(vec![text("chunk")]),
            ),
        );
        dispatch_provider_tool_update(
            &events,
            "session",
            acp::ToolCallUpdate::new(
                "partial-raw",
                acp::ToolCallUpdateFields::new().raw_input(json!({"x":1})),
            ),
        );
        dispatch_provider_tool_update(
            &events,
            "session",
            acp::ToolCallUpdate::new("empty", acp::ToolCallUpdateFields::new()),
        );
        dispatch_plan(&events, "session", acp::Plan::new(Vec::new()));
        dispatch_plan(
            &events,
            "session",
            acp::Plan::new(vec![
                acp::PlanEntry::new(
                    "pending",
                    acp::PlanEntryPriority::Low,
                    acp::PlanEntryStatus::Pending,
                ),
                acp::PlanEntry::new(
                    "active",
                    acp::PlanEntryPriority::Low,
                    acp::PlanEntryStatus::InProgress,
                ),
            ]),
        );

        let messages = collect_messages(receiver).await;
        assert_dispatched_messages(&messages);
    }

    #[tokio::test]
    async fn dispatches_each_optional_update_field_independently() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let updates = [
            acp::ToolCallUpdateFields::new()
                .title("Minimal")
                .status(acp::ToolCallStatus::Pending)
                .raw_input(json!({"minimal":true})),
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Pending)
                .raw_input(json!({"pending":true})),
            acp::ToolCallUpdateFields::new()
                .title("No input")
                .status(acp::ToolCallStatus::InProgress),
            acp::ToolCallUpdateFields::new()
                .title("Completed")
                .status(acp::ToolCallStatus::Completed)
                .raw_input(json!({"done":true}))
                .raw_output(json!("output")),
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Failed)
                .content(vec![text("failure")]),
            acp::ToolCallUpdateFields::new()
                .locations(vec![acp::ToolCallLocation::new("only-location")]),
        ];
        dispatch_optional_updates(&events, updates);

        let messages = collect_messages(receiver).await;
        // Location-only patches and content-only updates without status are dropped.
        assert_eq!(messages.len(), 5);
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-0")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-1")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-2")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-3")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-4")
        );
        assert!(
            !messages
                .iter()
                .any(|event| event["params"]["callId"] == "optional-5")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["output"] == "output")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["status"] == "failed")
        );
        assert!(!messages.iter().any(is_only_location));
    }

    #[tokio::test]
    async fn attaches_one_provenance_record_only_after_explicit_web_completion() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let evidence = ProviderWebEvidence::default();
        dispatch_provider_tool_call_with_evidence(
            &events,
            Some(&evidence),
            "session",
            acp::ToolCall::new("native-search", "WebSearch")
                .kind(acp::ToolKind::Search)
                .raw_input(json!({"query":"AVITA"})),
        );
        dispatch_web_completion_updates(&events, &evidence);
        dispatch_provider_tool_update_with_evidence(
            &events,
            Some(&evidence),
            "session",
            acp::ToolCallUpdate::new(
                "workspace-search",
                acp::ToolCallUpdateFields::new()
                    .title("Search workspace")
                    .kind(acp::ToolKind::Search)
                    .status(acp::ToolCallStatus::Completed)
                    .raw_output(json!("https://model.example/should-not-count")),
            ),
        );

        let messages = collect_messages(receiver).await;
        let completions = messages
            .iter()
            .filter(|message| message["params"]["evidence"]["verified"] == true)
            .collect::<Vec<_>>();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0]["params"]["evidence"]["kind"], "web_search");
        assert_eq!(
            completions[0]["params"]["evidence"]["source_urls"],
            json!(["https://example.com/result"])
        );
    }

    #[test]
    fn completed_evidence_metadata_is_added_only_for_valid_output() {
        let events = ThreadEventDispatcher::default();
        let evidence = ProviderWebEvidence::default();
        dispatch_provider_tool_call_with_evidence(
            &events,
            Some(&evidence),
            "session",
            acp::ToolCall::new("completed", "WebSearch")
                .kind(acp::ToolKind::Search)
                .status(acp::ToolCallStatus::Completed)
                .raw_input(json!({"query":"bounded test"}))
                .raw_output(json!("https://example.com/result"))
                .content(vec![text("result")]),
        );
        dispatch_provider_tool_call_with_evidence(
            &events,
            Some(&evidence),
            "session",
            acp::ToolCall::new("invalid", "WebSearch")
                .kind(acp::ToolKind::Search)
                .status(acp::ToolCallStatus::Completed)
                .raw_input(json!({"query":"bounded test"}))
                .raw_output(json!("not a url")),
        );
    }

    fn assert_dispatched_messages(messages: &[Value]) {
        assert_eq!(
            messages[0]["params"]["arguments"]["description"],
            "Edit file"
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "pending")
        );
        // Status-less content patches are not bridge events.
        assert!(
            !messages
                .iter()
                .any(|event| event["params"]["callId"] == "partial")
        );
        assert!(
            !messages
                .iter()
                .any(|event| event["params"]["callId"] == "partial-raw")
        );
        assert!(
            !messages
                .iter()
                .any(|event| event["params"]["callId"] == "empty")
        );
        assert!(messages.iter().any(|event| {
            event["params"]["delta"]
                .as_str()
                .is_some_and(|text| text.contains("Plan 0/2"))
        }));
    }

    fn text(value: &str) -> acp::ToolCallContent {
        acp::ContentBlock::Text(acp::TextContent::new(value)).into()
    }
}
