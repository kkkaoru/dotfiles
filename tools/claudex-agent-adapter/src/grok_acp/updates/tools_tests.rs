#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
            ("", "Tool"),
        ] {
            assert_eq!(
                tool_display_name(&acp::ToolCall::new("id", title)),
                expected
            );
        }
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

        let mut messages = Vec::new();
        while let Ok(Some(message)) =
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv()).await
        {
            messages.push(message);
        }
        assert_dispatched_messages(&messages);
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
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "partial")
        );
        assert!(
            messages
                .iter()
                .any(|event| event["params"]["callId"] == "partial-raw")
        );
        assert!(
            !messages
                .iter()
                .any(|event| event["params"]["callId"] == "empty")
        );
    }

    fn text(value: &str) -> acp::ToolCallContent {
        acp::ContentBlock::Text(acp::TextContent::new(value)).into()
    }
}
