use std::{convert::Infallible, ops::ControlFlow};

use anyhow::anyhow;
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{
    SegmentBuilder, error_flow, message_start, parse_tool_call, send_stream_completion,
    send_stream_error, send_stream_frame, tool_use_frames, turn_flow,
};

#[tokio::test]
async fn ignores_missing_and_empty_text_deltas() {
    let mut builder = SegmentBuilder::new(7);
    builder
        .text_delta(&json!({"params":{}}), None)
        .await
        .expect("missing delta");
    builder
        .text_delta(&json!({"params":{"delta":""}}), None)
        .await
        .expect("empty delta");
    let segment = builder.finish(None).await.expect("empty segment");
    assert!(segment.blocks.is_empty());
    assert_eq!(segment.usage.input_tokens, 7);
    assert_eq!(segment.usage.output_tokens, 0);
}

#[tokio::test]
async fn joins_text_deltas_and_estimates_usage() {
    let mut builder = SegmentBuilder::new(2);
    for delta in ["hello ", "world"] {
        builder
            .text_delta(&json!({"params":{"delta":delta}}), None)
            .await
            .expect("text delta");
    }
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{"inputTokens":9}}}
    }));
    let segment = builder.finish(None).await.expect("text segment");
    assert_eq!(segment.blocks[0]["text"], "hello world");
    assert_eq!(segment.stop_reason, "end_turn");
    assert_eq!(segment.usage.input_tokens, 9);
    assert!(segment.usage.output_tokens > 0);
}

#[tokio::test]
async fn streams_summarized_thinking_before_text_and_preserves_the_block() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(2);
    for (summary_index, delta) in [(0, "Plan"), (1, "Act")] {
        assert!(
            builder
                .model_output_event(
                    &json!({
                        "method":"item/reasoning/summaryTextDelta",
                        "params":{"itemId":"reasoning-1","summaryIndex":summary_index,"delta":delta}
                    }),
                    Some(&sender),
                )
                .await
                .expect("reasoning delta")
        );
    }
    assert!(
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/textDelta",
                    "params":{"itemId":"reasoning-1","contentIndex":0,"delta":"raw secret"}
                }),
                Some(&sender),
            )
            .await
            .expect("raw reasoning is ignored")
    );
    builder
        .text_delta(&json!({"params":{"delta":"Answer"}}), Some(&sender))
        .await
        .expect("text delta");
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{
            "inputTokens":9,"outputTokens":5,"reasoningOutputTokens":7
        }}}
    }));
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    assert_eq!(segment.blocks[0]["type"], "thinking");
    assert_eq!(segment.blocks[0]["thinking"], "Plan\n\nAct");
    assert!(
        segment.blocks[0]["signature"]
            .as_str()
            .is_some_and(|value| value.starts_with("claudex_local_"))
    );
    assert_eq!(segment.blocks[1], json!({"type":"text","text":"Answer"}));
    assert_eq!(segment.usage.input_tokens, 9);
    assert_eq!(segment.usage.output_tokens, 12);

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    assert_eq!(frames.len(), 8);
    assert_eq!(frames[0]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[1]["delta"],
        json!({"type":"thinking_delta","thinking":"Plan"})
    );
    assert_eq!(
        frames[2]["delta"],
        json!({"type":"thinking_delta","thinking":"\n\nAct"})
    );
    assert_eq!(frames[3]["delta"]["type"], "signature_delta");
    assert_eq!(frames[4], json!({"type":"content_block_stop","index":0}));
    assert_eq!(frames[5]["content_block"]["type"], "text");
    assert_eq!(
        frames[6]["delta"],
        json!({"type":"text_delta","text":"Answer"})
    );
    assert_eq!(frames[7], json!({"type":"content_block_stop","index":1}));
}

#[tokio::test]
async fn ignores_malformed_empty_raw_and_late_reasoning() {
    let mut builder = SegmentBuilder::new(1);
    for event in [
        json!({"method":"item/reasoning/summaryTextDelta","params":{}}),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0,"delta":""}
        }),
        json!({
            "method":"item/reasoning/textDelta",
            "params":{"itemId":"reasoning","contentIndex":0,"delta":"raw"}
        }),
    ] {
        assert!(
            builder
                .model_output_event(&event, None)
                .await
                .expect("ignored reasoning event")
        );
    }
    assert!(builder.blocks.is_empty());

    builder
        .text_delta(&json!({"params":{"delta":"visible"}}), None)
        .await
        .expect("visible text");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"late","summaryIndex":0,"delta":"late"}
            }),
            None,
        )
        .await
        .expect("late reasoning");
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.blocks, [json!({"type":"text","text":"visible"})]);
}

#[tokio::test]
async fn closes_each_reasoning_item_with_its_own_signature() {
    let mut builder = SegmentBuilder::new(1);
    for (item_id, delta) in [("first", "one"), ("second", "two")] {
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{"itemId":item_id,"summaryIndex":0,"delta":delta}
                }),
                None,
            )
            .await
            .expect("reasoning item");
    }
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.blocks.len(), 2);
    assert_eq!(segment.blocks[0]["thinking"], "one");
    assert_eq!(segment.blocks[1]["thinking"], "two");
    assert_ne!(
        segment.blocks[0]["signature"],
        segment.blocks[1]["signature"]
    );
    assert!(segment.usage.output_tokens > 0);
}

#[test]
fn parses_tool_calls_and_reports_each_missing_field() {
    let valid = json!({
        "id":8,
        "params":{"callId":"call","tool":"lookup"}
    });
    let call = parse_tool_call(&valid).expect("valid tool call");
    assert_eq!(call.call_id, "call");
    assert_eq!(call.name, "lookup");
    assert_eq!(call.arguments, &Value::Null);
    assert_eq!(call.request_id, json!(8));

    for (event, message) in [
        (json!({}), "params missing"),
        (json!({"params":{"tool":"x"},"id":1}), "callId missing"),
        (json!({"params":{"callId":"x"},"id":1}), "name missing"),
        (
            json!({"params":{"callId":"x","tool":"y"}}),
            "request id missing",
        ),
    ] {
        let error = match parse_tool_call(&event) {
            Ok(_) => panic!("invalid tool call was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(message));
    }
}

#[test]
fn handles_all_turn_and_error_states() {
    assert_eq!(
        turn_flow(&json!({})).expect("missing status"),
        ControlFlow::Break(())
    );
    assert_eq!(
        turn_flow(&json!({"params":{"turn":{"status":"inProgress"}}})).expect("in progress"),
        ControlFlow::Continue(())
    );
    assert!(
        turn_flow(&json!({"params":{"turn":{"status":"cancelled"}}}))
            .expect_err("failed status")
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(
        error_flow(&json!({"params":{"willRetry":true}})).expect("retry"),
        ControlFlow::Continue(())
    );
    assert!(error_flow(&json!({"params":{"message":"fatal"}})).is_err());
    assert!(error_flow(&json!({"message":"fatal"})).is_err());
}

#[tokio::test]
async fn emits_completion_error_and_optional_frames() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let segment = super::super::Segment {
        blocks: Vec::new(),
        stop_reason: "end_turn",
        usage: super::super::Usage {
            input_tokens: 1,
            output_tokens: 4,
        },
    };
    send_stream_completion(&sender, &segment).await;
    send_stream_error(&sender, anyhow!("boom")).await;
    send_stream_frame(None, "ignored", json!({}))
        .await
        .expect("optional stream");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("\"output_tokens\":4"));
    assert!(output.contains("event: message_stop"));
    assert!(output.contains("event: error"));
    assert!(output.contains("boom"));
}

#[test]
fn creates_start_and_tool_frames() {
    let start = message_start("test-model", 12);
    assert!(start.contains("\"model\":\"test-model\""));
    assert!(start.contains("\"input_tokens\":12"));
    let block = json!({
        "id":"toolu_test", "name":"lookup", "input":{"key":"value"}
    });
    let frames = tool_use_frames(3, &block);
    assert_eq!(frames[0].0, "content_block_start");
    assert_eq!(frames[1].1["index"], 3);
    assert!(
        frames[1].1["delta"]["partial_json"]
            .as_str()
            .expect("partial JSON")
            .contains("value")
    );
    assert_eq!(frames[2].0, "content_block_stop");
}
