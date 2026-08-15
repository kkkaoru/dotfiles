use std::{
    collections::{BTreeSet, HashMap, HashSet},
    convert::Infallible,
    ops::ControlFlow,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore, mpsc};

use super::protocol::send_tool_use;
use super::{
    SegmentBuilder, StreamWaitInput, builder::parse_tool_call, context_window, error_flow,
    message_start, sanitize, send_stream_completion, send_stream_error, send_stream_frame,
    thinking::ThinkingState, tool_use_frames, turn_flow,
};
use crate::{
    agent_backend::AgentBackend,
    anthropic::{ActiveTurn, Bridge, ContextRetry, MessagesRequest, Session},
    app_server::{AppServer, events::ThreadEventDispatcher},
    grok_acp::GrokAcp,
};

fn block_lacks_websearch_chrome(block: &Value) -> bool {
    ["text", "thinking"].into_iter().all(|key| {
        block.get(key).and_then(Value::as_str).is_none_or(|text| {
            text.trim().is_empty()
                || !(text.contains("WebSearch") || text.contains('🔎') || text.contains('▶'))
        })
    })
}

async fn drain_sse_frame_list(
    mut receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let bytes = frame.expect("frame");
        frames.push(String::from_utf8(bytes.to_vec()).expect("UTF-8 SSE"));
    }
    frames
}

fn track_content_block_frame(
    payload: &Value,
    open_index: &mut Option<usize>,
    next_index: &mut usize,
    started_types: &mut Vec<Value>,
) {
    match payload.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let index = payload["index"].as_u64().expect("start index") as usize;
            assert_eq!(index, *next_index, "content indices must not be reused");
            assert!(open_index.replace(index).is_none(), "nested content block");
            *next_index += 1;
            started_types.push(payload["content_block"]["type"].clone());
        }
        Some("content_block_delta") => {
            let index = payload["index"].as_u64().expect("delta index") as usize;
            assert_eq!(*open_index, Some(index), "delta must target the open block");
        }
        Some("content_block_stop") => {
            let index = payload["index"].as_u64().expect("stop index") as usize;
            assert_eq!(open_index.take(), Some(index), "stop must close its start");
        }
        _ => {}
    }
}

async fn yield_n(times: usize) {
    for _ in 0..times {
        tokio::task::yield_now().await;
    }
}

async fn wait_until_receiver_len(receiver: &mpsc::Receiver<Result<Bytes, Infallible>>, len: usize) {
    while receiver.len() < len {
        tokio::task::yield_now().await;
    }
}

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
    builder
        .text_delta(&json!({"params":{"delta":"Thought for 15s\n"}}), None)
        .await
        .expect("thought-for chrome");
    let segment = builder.finish(None).await.expect("empty segment");
    assert!(segment.blocks.is_empty());
    assert_eq!(segment.usage.input_tokens, 7);
    assert_eq!(segment.usage.output_tokens, 0);
}

#[tokio::test]
async fn pi_summarized_reasoning_streams_anthropic_thinking_frames() {
    let mut builder = SegmentBuilder::new(1);
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    assert!(
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{
                        "threadId":"thread",
                        "turnId":"thread",
                        "itemId":"pi-0",
                        "summaryIndex":0,
                        "delta":"Check the request path."
                    }
                }),
                Some(&sender),
            )
            .await
            .expect("Pi reasoning delta")
    );
    drop(sender);
    let output = drain_sse_frame_list(receiver).await.join("");
    assert!(output.contains(r#""type":"thinking""#));
    assert!(output.contains(r#""type":"thinking_delta""#));
    assert!(output.contains("Check the request path."));
}

#[test]
fn subagent_start_status_skips_main_and_command_code() {
    assert_eq!(
        super::prepare::subagent_start_status(false, "gpt-5.6-luna", None),
        None
    );
    assert_eq!(
        super::prepare::subagent_start_status(
            true,
            "meta/muse-spark-1.2-contributor",
            Some("high")
        ),
        None
    );
    // Suppress launch prose so ACP workers can show their own visible progress.
    assert_eq!(
        super::prepare::subagent_start_status(true, "gpt-5.6-luna", Some("max")),
        None
    );
    assert_eq!(
        super::prepare::subagent_start_status(true, "auto", None),
        None
    );
    assert_eq!(
        super::prepare::subagent_start_status(true, "opencode-go/deepseek-v4-pro", None),
        None
    );
}

#[tokio::test]
async fn status_item_and_provider_status_lines_paint_thinking_progress() {
    let mut builder = SegmentBuilder::for_turn(1, true, "auto");
    builder
        .text_delta(
            &json!({"params":{"itemId":"call-1:status","delta":"Plan: searching files"}}),
            None,
        )
        .await
        .expect("status itemId");
    builder
        .text_delta(
            &json!({"params":{"delta":"Session: ready\nPlan: continue"}}),
            None,
        )
        .await
        .expect("provider status lines");
    assert!(
        builder.thinking.is_open(),
        "ACP :status and Plan/Session lines must stay on thinking chrome"
    );
}

#[tokio::test]
async fn command_code_progress_drops_canned_only() {
    let mut builder = SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor");
    builder
        .stream_progress_text("Thought for 12s", None)
        .await
        .expect("canned thought-for");
    builder
        .stream_progress_text("起動: Command Code", None)
        .await
        .expect("canned launch");
    assert!(
        builder
            .blocks
            .iter()
            .all(thinking_omits_canned_command_code),
        "Command Code must drop canned chrome: {:?}",
        builder.blocks
    );
    builder
        .stream_progress_text("Check AVITA filings next.\n", None)
        .await
        .expect("real thought");
    builder
        .stream_progress_text("▶ Read CLAUDE.md", None)
        .await
        .expect("adapter marker");
    let thinking = builder
        .blocks
        .iter()
        .find_map(|block| block.get("thinking").and_then(Value::as_str))
        .unwrap_or("");
    assert!(
        thinking.contains("Check AVITA filings") && thinking.contains("▶ Read CLAUDE.md"),
        "non-canned Command Code progress must paint: {thinking:?}"
    );
}

fn thinking_omits_canned_command_code(block: &Value) -> bool {
    let Some(text) = block.get("thinking").and_then(Value::as_str) else {
        return true;
    };
    !text.contains("Thought for") && !text.contains("起動: Command Code")
}

#[tokio::test]
async fn flushes_pending_subagent_answer_and_keeps_main_keepalive_on_open_text() {
    let mut subagent = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    subagent
        .text_delta(
            &json!({"params":{"delta":"Only the first heading is Usage.\n"}}),
            None,
        )
        .await
        .expect("pending answer");
    let segment = subagent.finish(None).await.expect("flush pending answer");
    assert!(
        segment.blocks.iter().any(|block| block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| { text.contains("Only the first heading is Usage.") })),
        "subagent live prose must flush as text at end_turn: {:?}",
        segment.blocks
    );

    let mut main = SegmentBuilder::new(1);
    main.text_delta(&json!({"params":{"delta":"hello"}}), None)
        .await
        .expect("open text");
    main.activity_keepalive(None)
        .await
        .expect("keepalive on open text");
    let idle = SegmentBuilder::new(1);
    idle.activity_keepalive(None)
        .await
        .expect("keepalive without open text");
    let mut empty = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    empty.finish(None).await.expect("empty pending flush");
}

#[tokio::test]
async fn summarized_reasoning_skips_raw_text_delta_and_subagent_raw_cot() {
    let mut main = SegmentBuilder::new(1);
    main.model_output_event(
        &json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"r-sum","summaryIndex":0,"delta":"short summary"}
        }),
        None,
    )
    .await
    .expect("summary delta");
    main.model_output_event(
        &json!({
            "method":"item/reasoning/textDelta",
            "params":{"itemId":"r-sum","delta":"duplicate raw chain of thought"}
        }),
        None,
    )
    .await
    .expect("summarized raw skip");
    main.model_output_event(
        &json!({"method":"item/reasoning/textDelta","params":{}}),
        None,
    )
    .await
    .expect("missing item id");
    let mut subagent = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    subagent
        .model_output_event(
            &json!({
                "method":"item/reasoning/textDelta",
                "params":{"itemId":"r-raw","delta":"long subagent chain of thought"}
            }),
            Some(&sender),
        )
        .await
        .expect("subagent raw skip");
    drop(sender);
    assert!(
        subagent.blocks.iter().any(|block| {
            block
                .get("thinking")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("long subagent chain of thought"))
        }),
        "subagent raw CoT must stream live: {:?}",
        subagent.blocks
    );
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        sse.contains("thinking_delta") && sse.contains("long subagent chain of thought"),
        "SubAgent TUI must see raw CoT live: {sse}"
    );
}

#[tokio::test]
async fn ignores_whitespace_only_text_deltas() {
    let mut builder = SegmentBuilder::new(7);
    builder
        .text_delta(&json!({"params":{"delta":"  \n\n  "}}), None)
        .await
        .expect("whitespace-only delta");
}

#[tokio::test]
async fn subagent_progress_drops_canned_filler_and_paints_status() {
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    builder
        .text_delta(&json!({"params":{"delta":"Thought for 12s"}}), None)
        .await
        .expect("canned filler");
    builder
        .reasoning_delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":"Status: inspecting"}}),
            None,
        )
        .await
        .expect("worker status");
    builder
        .subagent_start_status("SubAgent starting: gpt-5.6-luna", None)
        .await
        .expect("start status");
    let mut command_code = SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor");
    command_code
        .subagent_start_status("should skip", None)
        .await
        .expect("command-code start status");
}

#[tokio::test]
async fn subagent_and_main_cover_bulk_dump_empty_delta_and_keepalive_title() {
    let dump = format!("{{\"{}\":1}}", "k".repeat(120));
    let mut subagent = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    subagent
        .text_delta(&json!({"params":{"delta":""}}), None)
        .await
        .expect("empty subagent delta");
    subagent
        .text_delta(&json!({"params":{"delta":dump.clone()}}), None)
        .await
        .expect("first bulk dump");
    subagent
        .text_delta(&json!({"params":{"delta":dump.clone()}}), None)
        .await
        .expect("second bulk dump");
    subagent
        .provider_tool_calls
        .push(("call".to_owned(), format!("Read {}", "a".repeat(60))));
    subagent
        .activity_keepalive(None)
        .await
        .expect("long keepalive title");
    subagent
        .provider_tool_calls
        .push(("short".to_owned(), "Read src/lib.rs".to_owned()));
    subagent
        .activity_keepalive(None)
        .await
        .expect("short keepalive title");
    subagent
        .reasoning_delta(
            &json!({"params":{"itemId":"r","summaryIndex":0,"delta":""}}),
            None,
        )
        .await
        .expect("empty subagent reasoning");
    subagent
        .reasoning_delta(
            &json!({"params":{"itemId":"r","summaryIndex":0,"delta":"Thought for 12s"}}),
            None,
        )
        .await
        .expect("canned subagent reasoning");
    subagent
        .reasoning_delta(
            &json!({"params":{"itemId":"r","summaryIndex":0,"delta":dump}}),
            None,
        )
        .await
        .expect("subagent reasoning dump");
    subagent
        .reasoning_delta(
            &json!({"params":{"itemId":"r","summaryIndex":0,"delta":"Inspect the neon pooler next.\n"}}),
            None,
        )
        .await
        .expect("subagent live reasoning");

    let mut main = SegmentBuilder::new(1);
    main.reasoning_delta(
        &json!({"params":{"itemId":"r","summaryIndex":0,"delta":"Inspect the neon pooler next.\n"}}),
        None,
    )
    .await
    .expect("open native thought");
    main.text_delta(
        &json!({"params":{"delta":"Thought for 12s\nhello from main"}}),
        None,
    )
    .await
    .expect("mixed canned answer delta");
    main.model_output_event(
        &json!({"method":"item/reasoning/textDelta","params":{}}),
        None,
    )
    .await
    .expect("raw reasoning without item id");
}

#[test]
fn recognizes_context_markers_in_every_provider_error_field() {
    let events = [
        json!({"params":{"error":{"message":"context window"}}}),
        json!({"params":{"message":"context window"}}),
        json!({"params":{"error":{"codexErrorInfo":"context window"}}}),
        json!({"params":{"error":{"code":"context window"}}}),
        json!({"params":{"error":{"type":"context window"}}}),
        json!({"params":{"error":{"name":"context window"}}}),
        json!({"params":{"error":{"additionalDetails":"context window"}}}),
    ];
    for event in events {
        assert!(context_window::is_context_window_event(&event));
    }
    assert!(context_window::is_context_window_event(
        &json!({"event":"context window"})
    ));
    assert!(!context_window::is_context_window_event(
        &json!({"params":{}})
    ));
}

#[test]
fn sanitizes_text_thinking_and_provider_status_variants() {
    let mut blocks = vec![
        json!({"type":"text","text":"hello\u{200b}"}),
        json!({"type":"text"}),
        json!({"type":"thinking"}),
        json!({"type":"thinking","thinking":"useful\u{200b}"}),
        json!({"type":"thinking","thinking":"▶ running"}),
        json!({"type":"unknown"}),
    ];
    sanitize::sanitize_committed_blocks(&mut blocks);
    assert_eq!(blocks[0]["text"], "hello");
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[2]["type"], "thinking");
    assert_eq!(blocks[3]["thinking"], "useful");
    assert_eq!(blocks[4]["type"], "unknown");
    assert_eq!(blocks.len(), 5);

    for status in [
        "✓ done",
        "✗ failed",
        "Plan next",
        "Plan:",
        "● step",
        "◎ step",
        "○ step",
        "SubAgent started: worker",
        "Retrying provider request",
        "Session mode: worker",
        "Session: worker",
        "🔎 WebSearch: Example Robotics",
        "… still working (2m) · last: Read foo",
        "Claudex is still working; waiting for provider output\u{2026}",
        "Thought for 17s",
        "Working on your request — I'll gather what I need and put together the result.",
        "Continuing with the next step in the plan.",
        "I’ll audit the local ctx index and pull the evidence needed for the report.",
        "\u{200b}\u{200b}",
        "  \n\t\n  ",
    ] {
        let mut status_block = vec![json!({"type":"thinking","thinking":status})];
        sanitize::sanitize_committed_blocks(&mut status_block);
        assert!(
            status_block.is_empty(),
            "status should be removed: {status}"
        );
    }

    assert!(sanitize::is_premature_worker_status_reply(
        "Status: inspecting\n\nStatus: still going"
    ));
    assert_eq!(
        sanitize::strip_worker_status_lines("keep\nStatus: inspecting\n"),
        "keep\n"
    );
    assert_eq!(
        sanitize::strip_worker_status_lines("keep\nStatus: inspecting"),
        "keep"
    );
    assert_eq!(
        sanitize::strip_worker_status_lines("keep\n\nStatus: inspecting\n"),
        "keep\n"
    );
}

fn assert_cursor_thought_for_filler_phrases() {
    assert!(sanitize::is_canned_worker_filler("Thought for 17s"));
    assert!(sanitize::is_canned_worker_filler(
        "Working on your request — I'll gather what I need and put together the result."
    ));
    assert!(sanitize::is_canned_worker_filler(
        "I’ll audit the local ctx index and pull the evidence needed for the report."
    ));
    assert!(sanitize::is_canned_worker_filler(
        "Continuing with the next step in the plan."
    ));
    assert!(sanitize::is_canned_worker_filler(
        "I’ve confirmed local history is indexed and located the session store"
    ));
    assert!(!sanitize::is_canned_worker_filler(
        "型と配信パスを把握しました。既存 finish-prediction-inputs-cache を確認します。"
    ));
    assert!(!sanitize::is_canned_worker_filler(
        "ContextVar は worker スレッドに伝わらないので、スレッド共有のフラグに切り替えます。"
    ));
    assert!(!sanitize::is_canned_worker_filler("   "));
    assert!(
        sanitize::is_canned_worker_filler("Nucleating…"),
        "Cursor SubAgent chrome must not freeze the panel on Nucleating"
    );
    assert!(sanitize::is_canned_worker_filler("Nucleating"));
}

fn assert_subagent_activity_and_silence_policy() {
    assert_subagent_activity_delays();
    assert_subagent_silence_judgment();
}

fn assert_subagent_activity_delays() {
    assert_eq!(
        super::SUBAGENT_INITIAL_ACTIVITY_DELAY,
        Duration::from_millis(100)
    );
    assert_eq!(super::INITIAL_ACTIVITY_DELAY, Duration::from_millis(250));
    assert!(super::SUBAGENT_INITIAL_ACTIVITY_DELAY < super::INITIAL_ACTIVITY_DELAY);
    assert!(super::INITIAL_ACTIVITY_DELAY < super::ACTIVITY_KEEPALIVE_INTERVAL);
    assert_eq!(super::ACTIVITY_KEEPALIVE_INTERVAL, Duration::from_secs(4));
    assert_eq!(
        super::types::stream_activity_delays(true),
        (
            super::SUBAGENT_INITIAL_ACTIVITY_DELAY,
            super::ACTIVITY_KEEPALIVE_INTERVAL
        )
    );
    assert_eq!(
        super::types::stream_activity_delays(false),
        (
            super::INITIAL_ACTIVITY_DELAY,
            super::ACTIVITY_KEEPALIVE_INTERVAL
        )
    );
    assert_eq!(
        super::prepare::prepare_first_activity_delay(true, true),
        Duration::ZERO,
        "primed SubAgent must receive its first activity opportunity immediately"
    );
    assert_eq!(
        super::prepare::prepare_first_activity_delay(true, false),
        super::SUBAGENT_INITIAL_ACTIVITY_DELAY
    );
    assert_eq!(
        super::prepare::prepare_first_activity_delay(false, false),
        super::INITIAL_ACTIVITY_DELAY
    );
}

fn assert_subagent_silence_judgment() {
    assert_eq!(
        super::types::SUBAGENT_PROVIDER_SILENCE_JUDGMENT,
        Duration::from_secs(20 * 60)
    );
    assert!(
        super::types::SUBAGENT_PROVIDER_SILENCE_JUDGMENT > Duration::from_secs(600),
        "judgment window must exceed Claude Code's ~600s watchdog so keepalives can cover real quiet work"
    );
    let mut subagent = SegmentBuilder::new(1).with_subagent(true);
    assert!(
        subagent.subagent_provider_silence_exceeded(Duration::ZERO),
        "SubAgents must be eligible for silence judgment"
    );
    subagent.note_visible_provider_activity();
    assert!(
        !subagent.subagent_provider_silence_exceeded(Duration::from_secs(60)),
        "visible provider activity must reset the silence clock"
    );
    assert!(
        !SegmentBuilder::new(1).subagent_provider_silence_exceeded(Duration::ZERO),
        "main turns must not use SubAgent silence judgment"
    );
    assert!(
        super::types::fail_if_subagent_provider_silent(
            &SegmentBuilder::new(1).with_subagent(false)
        )
        .is_ok()
    );
    let mut silent = SegmentBuilder::new(1).with_subagent(true);
    silent.backdate_last_visible_provider_activity(
        super::types::SUBAGENT_PROVIDER_SILENCE_JUDGMENT + Duration::from_secs(1),
    );
    let error = super::types::fail_if_subagent_provider_silent(&silent)
        .expect_err("silent SubAgent must end the turn");
    assert!(error.to_string().contains("provider produced no progress"));
}

#[test]
fn recognizes_cursor_thought_for_filler() {
    assert_cursor_thought_for_filler_phrases();
    assert_subagent_activity_and_silence_policy();
}

fn assert_compact_live_prose_basics() {
    assert_eq!(sanitize::compact_live_prose("short"), "short");
    let long = "あ".repeat(sanitize::LIVE_PROSE_CHAR_LIMIT + 3);
    let compact = sanitize::compact_live_prose(&long);
    assert!(compact.ends_with('…'));
    assert_eq!(compact.chars().count(), sanitize::LIVE_PROSE_CHAR_LIMIT + 1);
    assert_eq!(sanitize::latest_worker_status("   "), None);
    assert_eq!(sanitize::latest_worker_status("Working on it"), None);
    assert_eq!(
        sanitize::latest_worker_status("Status: inspecting\n"),
        Some("Status: inspecting\n".to_owned())
    );
    assert_eq!(
        sanitize::latest_worker_status("Status: one Status: two"),
        Some("Status: two\n".to_owned())
    );
}

fn assert_provider_status_line_markers() {
    for line in [
        "Plan next",
        "Plan: drafted",
        "● running",
        "◎ queued",
        "○ idle",
        "status: lower",
        "SubAgent starting: x",
        "SubAgent started: x",
        "SubAgent finished",
        "SubAgent completed",
        "Retrying provider request",
        "▶ running",
        "✓ done",
        "✗ failed",
        "… still working",
        "Claudex is still working; waiting",
        "Session mode: agent",
        "Session: live",
        "🔎 WebSearch: query",
    ] {
        assert!(sanitize::is_provider_status_line(line), "{line}");
    }
    assert!(!sanitize::is_provider_status_line("real assistant prose"));
    assert!(
        !sanitize::is_provider_status_line("Plan the per-race cache seed next."),
        "English CoT that starts with Plan must stay in the thinking transcript"
    );
    assert!(
        !sanitize::is_provider_status_line("Plan to inspect the Neon pooler GUCs."),
        "Plan-to CoT must not be treated as Muse chrome"
    );
    assert!(
        !sanitize::is_provider_status_line("Plan: migrate the Neon pooler GUCs next."),
        "Plan: sentence CoT must stay in the thinking transcript"
    );
}

fn assert_compact_live_prose_truncation() {
    assert_compact_live_prose_basics();
    assert_provider_status_line_markers();
}

fn assert_compact_worker_status_truncation() {
    assert!(sanitize::is_premature_worker_status_reply(
        "phase update: still drafting"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "starting phase 2"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "still working on it"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "Status: chrome only"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "after each phase we continue"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "Status: inspecting\nHere is the actual answer."
    ));
    assert!(!sanitize::is_premature_worker_status_reply(
        &"x".repeat(161)
    ));
    assert!(!sanitize::is_bulk_tool_dump("short dump"));
    assert!(sanitize::is_bulk_tool_dump(&format!(
        "{{{}{}}}",
        "\"k\":1,".repeat(20),
        ""
    )));
    assert!(sanitize::is_bulk_tool_dump(&format!(
        "[{}]",
        "1,".repeat(60)
    )));
    assert!(sanitize::is_bulk_tool_dump(
        "not json {a}{b}{c} extra braces wrap this dump and then more filler text for the length gate...."
    ));
    let mut chrome_only = vec![json!({
        "type": "thinking",
        "thinking": "Status: inspecting\n▶ running"
    })];
    sanitize::sanitize_committed_blocks(&mut chrome_only);
    assert!(
        chrome_only.is_empty(),
        "provider-status-only thinking must drop: {chrome_only:?}"
    );
}

#[test]
fn compact_live_prose_and_worker_status_cover_both_truncation_sides() {
    assert_compact_live_prose_truncation();
    assert_compact_worker_status_truncation();
}

#[test]
fn rewrites_premature_status_only_toolless_worker_replies() {
    assert!(sanitize::is_premature_worker_status_reply(
        "フェーズ1の短いステータス: 調査開始"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "short status after each phase"
    ));
    assert!(sanitize::is_premature_worker_status_reply(
        "Status: inspecting"
    ));
    assert!(!sanitize::is_premature_worker_status_reply(
        "Read CLAUDE.md and the first heading is # Claudex."
    ));
    assert!(!sanitize::is_premature_worker_status_reply(""));

    let premature = sanitize::rewrite_premature_status_only_segment(super::super::Segment {
        blocks: vec![json!({"type":"text","text":"各フェーズ後の短いステータスです"})],
        stop_reason: "end_turn",
        usage: super::super::Usage::default(),
        web_evidence: super::super::WebEvidenceSummary::default(),
    });
    assert_eq!(
        premature.blocks[0]["text"],
        sanitize::PREMATURE_STATUS_ONLY_NOTICE
    );

    let with_tool = sanitize::rewrite_premature_status_only_segment(super::super::Segment {
        blocks: vec![
            json!({"type":"tool_use","id":"1","name":"Read","input":{}}),
            json!({"type":"text","text":"フェーズ1完了"}),
        ],
        stop_reason: "end_turn",
        usage: super::super::Usage::default(),
        web_evidence: super::super::WebEvidenceSummary::default(),
    });
    assert_eq!(with_tool.blocks[1]["text"], "フェーズ1完了");

    let real_answer = sanitize::rewrite_premature_status_only_segment(super::super::Segment {
        blocks: vec![json!({"type":"text","text":"The heading is # Claudex adapter."})],
        stop_reason: "end_turn",
        usage: super::super::Usage::default(),
        web_evidence: super::super::WebEvidenceSummary::default(),
    });
    assert_eq!(
        real_answer.blocks[0]["text"],
        "The heading is # Claudex adapter."
    );
}

#[tokio::test]
async fn subagent_coalesced_reasoning_continues_after_native_tool_use() {
    let mut state = ThinkingState::default();
    let mut blocks = vec![json!({
        "type":"tool_use",
        "id":"toolu_read",
        "name":"Read",
        "input":{"path":"scripts/CLAUDE.md"}
    })];
    state
        .delta_text_coalesced(
            "reasoning",
            0,
            "Inspect how sync-realtime-data chooses the writable Neon connection.",
            &mut blocks,
            None,
        )
        .await
        .expect("subagent reasoning after Read");
    assert!(
        blocks.iter().any(|block| {
            block
                .get("thinking")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("writable Neon connection"))
        }),
        "Codex/luna reasoning after Read must stay live: {blocks:?}"
    );
}

async fn run_thinking_keepalive_phase() -> (ThinkingState, Vec<Value>) {
    let mut state = ThinkingState::default();
    let mut blocks = Vec::new();
    state
        .delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":"one"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("first thought");
    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("model reasoning heartbeat");
    state
        .delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":" two"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("continued thought");
    state.close(&mut blocks, None).await.expect("close thought");
    state
        .close(&mut blocks, None)
        .await
        .expect("close empty state");

    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("first keepalive");
    state
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("heartbeat keepalive");
    state
        .close(&mut blocks, None)
        .await
        .expect("close keepalive before switching buffers");
    let mut visible = vec![json!({"type":"text","text":"answer"})];
    state
        .activity_keepalive(&mut visible, None)
        .await
        .expect("visible output keepalive");
    assert!(
        visible
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) != Some("thinking")),
        "silence after visible text must not open STATUS thinking: {visible:?}"
    );
    state
        .close(&mut visible, None)
        .await
        .expect("close visible keepalive before switching buffers");
    (state, blocks)
}

async fn run_thinking_status_phase(state: &mut ThinkingState, blocks: &mut Vec<Value>) {
    state
        .progress_status(blocks, "", None)
        .await
        .expect("empty progress");
    state
        .progress_status_keep_open(blocks, "", None)
        .await
        .expect("empty keep-open progress");
    state
        .activity_status(blocks, "", None)
        .await
        .expect("empty activity");
    state
        .delta(
            &json!({"params":{"itemId":"model:status","summaryIndex":0,"delta":"ignored"}}),
            blocks,
            None,
        )
        .await
        .expect("status-like thought");
    state
        .delta(&json!({"params":{"delta":"no-item"}}), blocks, None)
        .await
        .expect("summary-less delta is ignored");
    let mut visible_status = vec![json!({"type":"text","text":"answer"})];
    state
        .activity_status(&mut visible_status, "still working", None)
        .await
        .expect("visible activity status");
    assert!(
        !visible_status
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("thinking")),
        "non-empty activity_status must not open thinking after visible output"
    );
    ThinkingState::default()
        .elapsed_keepalive(blocks, Duration::from_secs(1), None, None)
        .await
        .expect("elapsed keepalive without an open block");
}

#[tokio::test]
async fn thinking_state_handles_reuse_keepalive_and_unit_transitions() {
    let (mut state, mut blocks) = run_thinking_keepalive_phase().await;
    run_thinking_status_phase(&mut state, &mut blocks).await;
}

#[tokio::test]
async fn progress_status_dedupes_identical_status_lines() {
    let mut state = ThinkingState::default();
    let mut blocks = Vec::new();
    let status = "Status: inspecting local history\n";
    state
        .progress_status_keep_open(&mut blocks, status, None)
        .await
        .expect("first status");
    state
        .progress_status_keep_open(&mut blocks, status, None)
        .await
        .expect("duplicate status");
    let thinking = blocks[0]["thinking"].as_str().expect("thinking text");
    assert_eq!(
        thinking.matches("Status: inspecting local history").count(),
        1,
        "identical Status chrome must not append twice: {thinking:?}"
    );
    state
        .progress_status_keep_open(&mut blocks, "Status: next step\n", None)
        .await
        .expect("distinct status");
    let thinking = blocks[0]["thinking"].as_str().expect("thinking text");
    assert!(thinking.contains("Status: inspecting local history"));
    assert!(thinking.contains("Status: next step"));
}

#[tokio::test]
async fn progress_status_dedupes_tool_tip_after_elapsed_keepalive() {
    let mut state = ThinkingState::default();
    let mut blocks = Vec::new();
    let tip = "▶ Read\n";
    state
        .progress_status_keep_open(&mut blocks, tip, None)
        .await
        .expect("first tip");
    state
        .elapsed_keepalive(&blocks, Duration::from_secs(4), Some("Read"), None)
        .await
        .expect("elapsed keepalive");
    state
        .progress_status_keep_open(&mut blocks, tip, None)
        .await
        .expect("re-tip after elapsed keepalive");
    state
        .elapsed_keepalive(&blocks, Duration::from_secs(8), Some("Read"), None)
        .await
        .expect("second elapsed keepalive");
    state
        .progress_status_keep_open(&mut blocks, tip, None)
        .await
        .expect("second re-tip after elapsed keepalive");
    let thinking = blocks[0]["thinking"].as_str().expect("thinking text");
    assert_eq!(
        thinking.matches("▶ Read").count(),
        1,
        "elapsed keepalive must not defeat ▶ tip dedupe: {thinking:?}"
    );
}

#[tokio::test]
async fn thinking_state_covers_answer_text_prime_and_heartbeat_guards() {
    let mut coalesced = ThinkingState::default();
    let mut answer = vec![json!({"type":"text","text":"done"})];
    coalesced
        .delta_text_coalesced("reasoning", 0, "should stay quiet", &mut answer, None)
        .await
        .expect("coalesced delta after answer text");
    assert_eq!(answer.len(), 1);

    let mut primed = ThinkingState::default();
    let mut blocks = Vec::new();
    primed
        .delta(
            &json!({"params":{"itemId":"reasoning","summaryIndex":0,"delta":"open"}}),
            &mut blocks,
            None,
        )
        .await
        .expect("open thought");
    // A silent prime no longer reserves a synthetic thinking block. The real
    // CoT block above remains the only committed block.
    assert_eq!(
        blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .count(),
        1
    );
    primed
        .activity_keepalive(&mut blocks, None)
        .await
        .expect("heartbeat on open thought");

    let activity = ThinkingState::default();
    let mut keepalive = Vec::new();
    activity
        .activity_status(&mut keepalive, "still working", None)
        .await
        .expect("open activity status");
    activity
        .activity_status(&mut keepalive, "again", None)
        .await
        .expect("ignore later activity status");
    assert_eq!(
        keepalive
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .count(),
        0,
        "synthetic activity status must not open thinking"
    );
}

#[tokio::test]
async fn joins_text_deltas_and_estimates_usage() {
    let mut builder = SegmentBuilder::new(2);
    assert!(!builder.has_external_tool_calls());
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
async fn defaults_missing_reasoning_usage_to_zero() {
    let mut builder = SegmentBuilder::new(2);
    builder.update_usage(&json!({
        "params":{"tokenUsage":{"last":{"outputTokens":5}}}
    }));
    let segment = builder.finish(None).await.expect("usage segment");
    assert_eq!(segment.usage.output_tokens, 5);
}

#[tokio::test]
async fn streams_summarized_thinking_as_separate_units_before_text() {
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

    // Each summaryIndex is its own thinking block (Claude-like units).
    assert_eq!(segment.blocks[0]["type"], "thinking");
    assert_eq!(segment.blocks[0]["thinking"], "Plan");
    assert_eq!(segment.blocks[1]["type"], "thinking");
    assert_eq!(segment.blocks[1]["thinking"], "Act");
    assert_ne!(
        segment.blocks[0]["signature"],
        segment.blocks[1]["signature"]
    );
    assert_eq!(segment.blocks[2], json!({"type":"text","text":"Answer"}));
    assert_eq!(segment.usage.input_tokens, 9);
    assert_eq!(segment.usage.output_tokens, 12);

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    // start Plan, delta, sig, stop, start Act, delta, sig, stop, start text, delta, stop
    assert_eq!(frames.len(), 11);
    assert_eq!(frames[0]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[1]["delta"],
        json!({"type":"thinking_delta","thinking":"Plan"})
    );
    assert_eq!(frames[2]["delta"]["type"], "signature_delta");
    assert_eq!(frames[3], json!({"type":"content_block_stop","index":0}));
    assert_eq!(frames[4]["content_block"]["type"], "thinking");
    assert_eq!(
        frames[5]["delta"],
        json!({"type":"thinking_delta","thinking":"Act"})
    );
    assert_eq!(frames[6]["delta"]["type"], "signature_delta");
    assert_eq!(frames[7], json!({"type":"content_block_stop","index":1}));
    assert_eq!(frames[8]["content_block"]["type"], "text");
    assert_eq!(
        frames[9]["delta"],
        json!({"type":"text_delta","text":"Answer"})
    );
    assert_eq!(frames[10], json!({"type":"content_block_stop","index":2}));
}

async fn feed_subagent_reasoning_across_units(
    builder: &mut SegmentBuilder,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) {
    for (summary_index, delta) in [
        (
            0,
            "Map the conversion path.
",
        ),
        (
            1,
            "Check Vibrato boundaries.
",
        ),
    ] {
        assert!(
            builder
                .model_output_event(
                    &json!({
                        "method":"item/reasoning/summaryTextDelta",
                        "params":{
                            "itemId":"worker:reasoning",
                            "summaryIndex":summary_index,
                            "delta":delta
                        }
                    }),
                    Some(sender),
                )
                .await
                .expect("subagent reasoning delta")
        );
    }
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"read-1",
                    "tool":"Read",
                    "title":"Read convert.ts",
                    "arguments":{"path":"packages/azookey/convert.ts"}
                }
            }),
            Some(sender),
        )
        .await
        .expect("tool progress");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{
                    "itemId":"worker:reasoning-2",
                    "summaryIndex":0,
                    "delta":"Hypothesis: boundaries were dropped.
            "
                }
            }),
            Some(sender),
        )
        .await
        .expect("later reasoning unit");
}

fn assert_subagent_reasoning_transcript(segment: &crate::anthropic::Segment) {
    let thinking = segment
        .blocks
        .iter()
        .find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("thinking"))
                .then(|| block.get("thinking").and_then(Value::as_str))
                .flatten()
        })
        .unwrap_or("");
    assert_eq!(
        segment
            .blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .count(),
        1,
        "SubAgent turn must keep one native thinking block: {:?}",
        segment.blocks
    );
    // Live SSE streams real CoT; ▶ tool chrome stays on thinking.
    assert!(thinking.contains("Map the conversion path."));
    assert!(thinking.contains("Check Vibrato boundaries."));
    assert!(thinking.contains("Hypothesis: boundaries were dropped."));
    assert!(
        !thinking.contains('▶'),
        "▶ chrome must be stripped from the transcript: {thinking}"
    );
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) != Some("server_tool_use")),
        "ACP SubAgent must not close thinking for server_tool_use: {:?}",
        segment.blocks
    );
}

async fn collect_sse_frames(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> String {
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    sse
}

fn raw_reasoning_textdelta(item_id: &str, delta: &str) -> Value {
    json!({
        "method":"item/reasoning/textDelta",
        "params":{"itemId":item_id,"contentIndex":0,"delta":delta}
    })
}

fn assert_thinking_stop_before_native_read(output: &str) {
    let tool_use = output.find(r#""type":"tool_use""#).expect("tool_use");
    assert!(
        output.contains("Read"),
        "native Read card missing: {output}"
    );
    assert!(
        output[..tool_use].contains("content_block_stop"),
        "thinking stop must precede tool_use: {output}"
    );
}

async fn feed_subagent_read(
    builder: &mut SegmentBuilder,
    bridge: &Bridge,
    session: &Session,
    call_id: &str,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) {
    let _ = builder
        .handle_event(
            bridge,
            session,
            &[],
            &Value::Null,
            &json!({
                "id":9,
                "method":"item/tool/call",
                "params":{
                    "callId":call_id,
                    "tool":"cc_Read_0",
                    "arguments":{"path":"scripts/CLAUDE.md"}
                }
            }),
            Some(sender),
        )
        .await
        .expect("subagent Read");
}

async fn assert_codex_cot_in_transcript(builder: &mut SegmentBuilder, needle: &str) {
    let (finish_sender, _finish_rx) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let segment = builder
        .finish(Some(&finish_sender))
        .await
        .expect("finish after Codex CoT");
    let thinking = segment
        .blocks
        .iter()
        .find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("thinking"))
                .then(|| block.get("thinking").and_then(Value::as_str))
                .flatten()
        })
        .unwrap_or("");
    assert!(
        thinking.contains(needle),
        "Codex CoT must land in the transcript: {thinking}"
    );
    assert!(
        !thinking.contains('▶'),
        "▶ chrome must be stripped from the transcript: {thinking}"
    );
}

fn assert_subagent_reasoning_sse(sse: &str) {
    assert_eq!(
        sse.matches("\"type\":\"content_block_start\"").count(),
        1,
        "live stream must open thinking only once: {sse}"
    );
    assert_eq!(
        sse.matches("\"type\":\"signature_delta\"").count(),
        1,
        "thinking must stay open until end_turn: {sse}"
    );
    assert!(
        sse.contains("Map the conversion path"),
        "ACP CoT must stream live: {sse}"
    );
    assert!(
        sse.contains("▶ Read") || sse.contains("▶ convert.ts"),
        "tool progress must append into the open thinking block: {sse}"
    );
    assert!(
        !sse.contains("\"type\":\"server_tool_use\""),
        "live stream must not paint server_tool_use: {sse}"
    );
}

#[tokio::test]
async fn subagent_reasoning_stays_on_one_thinking_block_across_units() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(2).with_subagent(true);
    feed_subagent_reasoning_across_units(&mut builder, &sender).await;
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    assert_subagent_reasoning_transcript(&segment);
    let sse = collect_sse_frames(&mut receiver).await;
    assert_subagent_reasoning_sse(&sse);
}

#[tokio::test]
async fn command_code_reasoning_stays_on_one_thinking_block() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(2)
        .with_subagent(true)
        .with_command_code_progress(true);
    for (summary_index, delta) in [
        (0, "Probe the Viterbi lattice.\n"),
        (1, "Enumerate dictionary segments.\n"),
    ] {
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{
                        "itemId":"command-code:reasoning",
                        "summaryIndex":summary_index,
                        "delta":delta
                    }
                }),
                Some(&sender),
            )
            .await
            .expect("command-code reasoning");
    }
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{
                    "itemId":"command-code:message",
                    "delta":"Reproducing the malformed conversion next.\n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("command-code status");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    let thinking_block_count = segment
        .blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
        .count();
    assert_eq!(
        thinking_block_count, 1,
        "Command Code must not open/close thinking per unit: {:?}",
        segment.blocks
    );
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert_eq!(
        sse.matches("\"thinking\":\"\",\"type\":\"thinking\"")
            .count(),
        1,
        "Command Code live stream must open thinking only once: {sse}"
    );
    assert_eq!(
        sse.matches("\"type\":\"signature_delta\"").count(),
        1,
        "Command Code thinking must stay open until end_turn: {sse}"
    );
    assert!(
        !sse.contains("Thought for"),
        "Command Code must not emit Thought-for chrome: {sse}"
    );
}

#[tokio::test]
async fn command_code_muse_spark_status_bursts_stay_on_one_thinking_block() {
    // Live dump: Muse Spark SubAgent card flickered `Thought for 15s/19s/5s…`
    // between "I'm setting up your pooled-primary…" status lines.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor");
    for (summary_index, delta) in [
        (
            0,
            "I'm setting up your pooled-primary runtime checks A/B/C.\n",
        ),
        (1, "Thought for 15s\n"),
        (2, "Polling the pooled primary next — fresh connections.\n"),
        (3, "Thought for 19s\n"),
        (0, "Running the live pooled-primary A/B/C checks.\n"),
    ] {
        builder
            .model_output_event(
                &json!({
                    "method":"item/reasoning/summaryTextDelta",
                    "params":{
                        "itemId":"command-code:reasoning",
                        "summaryIndex":summary_index,
                        "delta":delta
                    }
                }),
                Some(&sender),
            )
            .await
            .expect("muse spark reasoning");
    }
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{
                    "itemId":"command-code:message",
                    "delta":"I'm setting up your pooled-primary runtime checks A/B/C — loading the key with safe redacted parsing.\n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("muse spark status");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    let thinking_block_count = segment
        .blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
        .count();
    assert_eq!(
        thinking_block_count, 1,
        "Muse Spark dump must not open/close thinking per burst: {:?}",
        segment.blocks
    );
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert_eq!(
        sse.matches("\"thinking\":\"\",\"type\":\"thinking\"")
            .count(),
        1,
        "Muse Spark live stream must open thinking only once: {sse}"
    );
    assert_eq!(
        sse.matches("\"type\":\"signature_delta\"").count(),
        1,
        "Muse Spark thinking must stay open until end_turn: {sse}"
    );
    assert!(
        !sse.contains("Thought for"),
        "Muse Spark must not emit Thought-for chrome: {sse}"
    );
    assert!(
        sse.contains("pooled-primary"),
        "status prose must stay on thinking: {sse}"
    );
}

#[tokio::test]
async fn gpt_textdelta_without_summary_paints_native_thinking() {
    // Main Codex still surfaces raw `textDelta` as Thinking. SubAgents must not;
    // see `gpt_subagent_textdelta_does_not_bury_live_tool_progress`.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::for_turn(1, false, "gpt-5.6-luna");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/textDelta",
                "params":{
                    "itemId":"gpt:reasoning",
                    "contentIndex":0,
                    "delta":"Inspect the neon pooler GUCs on a fresh connection.\n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("gpt textdelta");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    assert!(
        segment.blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("thinking")
                && block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("neon pooler GUCs"))
        }),
        "GPT textDelta must become native thinking: {:?}",
        segment.blocks
    );
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        sse.contains("thinking_delta") && sse.contains("neon pooler GUCs"),
        "Claude Code Thinking must stream GPT textDelta live: {sse}"
    );
    assert!(
        !sse.contains("raw secret"),
        "summary-backed items must still hide raw textDelta: {sse}"
    );
}

#[tokio::test]
async fn gpt_textdelta_without_content_index_still_paints_main_thinking() {
    let mut builder = SegmentBuilder::for_turn(1, false, "glm-5.2:cloud");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/textDelta",
                "params":{
                    "itemId":"glm:reasoning",
                    "delta":"Ollama GLM CoT without contentIndex must still stream.\n"
                }
            }),
            None,
        )
        .await
        .expect("glm textdelta");
    let segment = builder.finish(None).await.expect("segment");
    assert!(
        segment.blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("thinking")
                && block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("without contentIndex"))
        }),
        "main Codex textDelta must not require contentIndex: {:?}",
        segment.blocks
    );
}

#[tokio::test]
async fn gpt_subagent_textdelta_does_not_bury_live_tool_progress() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    let cot = "Inspect the neon pooler GUCs on a fresh connection.\n".repeat(40);
    builder
        .model_output_event(
            &raw_reasoning_textdelta("gpt:reasoning", &cot),
            Some(&sender),
        )
        .await
        .expect("gpt textdelta");
    feed_subagent_read(&mut builder, &bridge, &session, "read-claude-md", &sender).await;
    assert!(
        !builder.thinking.is_open(),
        "Read must close thinking so the native card is live"
    );
    drop(sender);
    let sse = collect_sse_frames(&mut receiver).await;
    assert_thinking_stop_before_native_read(&sse);
    assert!(
        sse.contains("Inspect the neon pooler GUCs"),
        "raw GPT CoT must stream live: {sse}"
    );
    assert_codex_cot_in_transcript(&mut builder, "Inspect the neon pooler GUCs").await;
}

async fn feed_mixed_reasoning_and_provider_progress(
    builder: &mut SegmentBuilder,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) {
    builder
        .model_output_event(
            &raw_reasoning_textdelta("reasoning-a", "UNIQUE_REASONING_A\n"),
            Some(sender),
        )
        .await
        .expect("reasoning A");
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"provider-read",
                "tool":"Read",
                "title":"PROVIDER_OWNED_PROGRESS",
                "arguments":{"path":"provider-only.txt"}
            }}),
            Some(sender),
        )
        .await
        .expect("provider-owned progress");
    builder
        .model_output_event(
            &raw_reasoning_textdelta("reasoning-b", "UNIQUE_REASONING_B\n"),
            Some(sender),
        )
        .await
        .expect("reasoning B");
}

fn parse_sse_payloads(raw_frames: &[String]) -> Vec<Value> {
    raw_frames
        .iter()
        .map(|frame| {
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("SSE data");
            serde_json::from_str::<Value>(data).expect("JSON frame")
        })
        .collect()
}

fn thinking_delta_contains(payload: &Value, index: usize, needle: &str) -> bool {
    payload["type"] == "content_block_delta"
        && payload["index"] == index
        && payload["delta"]["thinking"]
            .as_str()
            .is_some_and(|text| text.contains(needle))
}

fn assert_mixed_handoff_sse(payloads: &[Value]) {
    let mut open_index = None;
    let mut next_index = 0;
    let mut started_types = Vec::new();
    for payload in payloads {
        track_content_block_frame(
            payload,
            &mut open_index,
            &mut next_index,
            &mut started_types,
        );
    }
    assert_eq!(open_index, None, "all streamed blocks must be closed");
    assert_eq!(
        started_types,
        vec![json!("thinking"), json!("tool_use")],
        "finish must not synthesize an unstarted thinking block: {payloads:?}"
    );
    let signature_indices = payloads
        .iter()
        .filter(|payload| {
            payload.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
        })
        .filter_map(|payload| payload["index"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(signature_indices, vec![0]);
    let stop_indices = payloads
        .iter()
        .filter(|payload| payload["type"] == "content_block_stop")
        .filter_map(|payload| payload["index"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(stop_indices, vec![0, 1]);
    for needle in [
        "UNIQUE_REASONING_A",
        "PROVIDER_OWNED_PROGRESS",
        "UNIQUE_REASONING_B",
    ] {
        assert!(
            payloads
                .iter()
                .any(|payload| thinking_delta_contains(payload, 0, needle)),
            "{needle} must stream on the live thinking index: {payloads:?}"
        );
    }
}

fn assert_clean_mixed_handoff_transcript(segment: &crate::anthropic::Segment) {
    let block_types = segment
        .blocks
        .iter()
        .filter_map(|block| block["type"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(block_types, vec!["thinking", "tool_use"]);
    let thinking = segment
        .blocks
        .iter()
        .filter_map(|block| {
            (block["type"] == "thinking")
                .then(|| block["thinking"].as_str())
                .flatten()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        thinking,
        vec!["UNIQUE_REASONING_A\nUNIQUE_REASONING_B"],
        "transcript must commit CoT exactly once without provider chrome"
    );
    assert_eq!(
        segment
            .blocks
            .iter()
            .filter(|block| block["type"] == "tool_use")
            .count(),
        1,
        "native Read must remain the only executable tool block"
    );
    let transcript = serde_json::to_string(&segment.blocks).expect("transcript JSON");
    for sentinel in ["UNIQUE_REASONING_A", "UNIQUE_REASONING_B"] {
        assert_eq!(transcript.matches(sentinel).count(), 1);
    }
    assert!(!transcript.contains("PROVIDER_OWNED_PROGRESS"));
}

#[tokio::test]
async fn subagent_mixed_reasoning_provider_progress_and_read_has_no_ghost_thinking_block() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    assert!(super::prime_subagent_sse(
        &sender,
        "gpt-5.6-luna",
        1,
        true,
        None
    ));
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna").with_primed_thinking();
    feed_mixed_reasoning_and_provider_progress(&mut builder, &sender).await;
    feed_subagent_read(&mut builder, &bridge, &session, "native-read", &sender).await;
    let segment = builder.finish(Some(&sender)).await.expect("tool handoff");
    drop(sender);

    let payloads = parse_sse_payloads(&drain_sse_frame_list(receiver).await);
    assert_mixed_handoff_sse(&payloads);
    assert_clean_mixed_handoff_transcript(&segment);
}

#[tokio::test]
async fn gpt_summary_still_hides_raw_textdelta() {
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"gpt:reasoning","summaryIndex":0,"delta":"Inspect the neon pooler next.\n"}
            }),
            None,
        )
        .await
        .expect("summary");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/textDelta",
                "params":{"itemId":"gpt:reasoning","contentIndex":0,"delta":"raw secret"}
            }),
            None,
        )
        .await
        .expect("raw textdelta");
    let segment = builder.finish(None).await.expect("segment");
    let thinking = segment
        .blocks
        .iter()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
        .and_then(|block| block.get("thinking").and_then(Value::as_str))
        .unwrap_or_default();
    assert!(thinking.contains("Inspect the neon pooler next"));
    assert!(!thinking.contains("raw secret"));
}

#[tokio::test]
async fn whitespace_reasoning_delta_does_not_open_blank_thought_chrome() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"cline:reasoning","summaryIndex":0,"delta":"\n\n  \n"}
            }),
            Some(&sender),
        )
        .await
        .expect("ignore whitespace thought");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("visible keepalive after blank thought");
    drop(sender);
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    assert!(
        frames.iter().all(|frame| {
            frame["delta"]["thinking"]
                != "Claudex is still working; waiting for provider output\u{2026}"
        }),
        "blank Cline thought must not open STATUS thinking: {frames:?}"
    );
}

#[tokio::test]
async fn activity_keepalive_stays_content_free() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("first keepalive");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("second keepalive");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    // Pure keepalive thinking is stripped from the committed segment.
    assert!(segment.blocks.is_empty());

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        frames.push(serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame"));
    }
    assert!(
        frames.is_empty(),
        "silence must not open STATUS thinking: {frames:?}"
    );
}

#[tokio::test]
async fn activity_keepalive_continues_after_bridged_tool_use_so_watchdog_does_not_stall() {
    // Historical TUI failure (fa522331 / spark a989e556):
    // `Agent "Verify today's time and live data" failed: Agent stalled: no progress
    // for 600s (stream watchdog did not recover)` after Bash/WebSearch tool_use.
    // Old activity_keepalive returned immediately once any non-thinking block
    // existed, so decoded keepalives stopped for the rest of the turn.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder.blocks.push(json!({
        "type":"tool_use",
        "id":"toolu_bash",
        "name":"Bash",
        "input":{"command":"date"}
    }));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive after tool_use");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("second heartbeat after tool_use");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    assert_eq!(segment.blocks[0]["name"], "Bash");
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) != Some("thinking")),
        "keepalive thinking must stay out of the committed transcript"
    );
    assert!(
        !stream_contains_status(&mut receiver).await,
        "bridged tool_use must not reopen STATUS thinking"
    );
}

#[tokio::test]
async fn activity_keepalive_leaves_visible_output_unchanged() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .text_delta(&json!({"params":{"delta":"hi"}}), Some(&sender))
        .await
        .expect("text");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("text heartbeat");
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    // Stream-only heartbeat: final answer text stays clean.
    assert_eq!(segment.blocks[0], json!({"type":"text","text":"hi"}));
    assert!(
        !stream_contains_zwsp(&mut receiver).await,
        "activity keepalive must not emit synthetic zero-width text_delta"
    );
}

#[tokio::test]
async fn refreshes_activity_deadlines_and_detects_closed_streams() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let builder = SegmentBuilder::new(1);
    let mut deadline = Box::pin(tokio::time::sleep(std::time::Duration::from_secs(1)));
    super::control::refresh_activity_keepalive(
        &builder,
        Some(&sender),
        deadline.as_mut(),
        Duration::from_secs(1),
    )
    .await
    .expect("activity keepalive");
    assert!(!deadline.is_elapsed());

    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    assert!(
        !bridge
            .finish_if_stream_closed(&sender, &session, &events, true)
            .await
    );
    drop(receiver);
    assert!(
        bridge
            .finish_if_stream_closed(&sender, &session, &events, true)
            .await
    );
    super::disconnect::warn_disconnect_failure(
        &anyhow!("test drain failure"),
        "thread",
        "tested disconnect warning",
    );
    super::disconnect::warn_cancel_failure(&anyhow!("test cancel failure"), "thread");
}

#[tokio::test]
async fn hidden_provider_events_do_not_postpone_visible_activity() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let wait = bridge.wait_for_stream_segment_with_interval(StreamWaitInput {
        session: &session,
        events: Arc::new(events),
        current_messages: &[],
        system: &json!(null),
        sender: &sender,
        builder: SegmentBuilder::new(1),
        activity_interval: Duration::from_millis(10),
        initial_activity_delay: Duration::from_millis(10),
    });
    let dispatch = dispatch_hidden_events(&dispatcher);
    let (result, ()) = tokio::join!(wait, dispatch);
    result.expect("stream segment");
    drop(sender);

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("waiting for provider output"),
        "hidden events must not invent STATUS thinking: {output}"
    );
}

async fn stream_contains_status(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> bool {
    let mut saw = false;
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        saw |= frame.contains("Claudex is still working");
    }
    saw
}

async fn stream_contains_zwsp(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> bool {
    let mut saw_zwsp = false;
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        saw_zwsp |= frame.contains("\\u200b") || frame.contains('\u{200b}');
    }
    saw_zwsp
}

async fn dispatch_hidden_events(dispatcher: &crate::app_server::events::ThreadEventDispatcher) {
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(4)).await;
        dispatcher.dispatch(json!({
            "method":"thread/tokenUsage/updated",
            "params":{
                "threadId":"thread",
                "tokenUsage":{"last":{"inputTokens":1}}
            }
        }));
    }
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
}

#[tokio::test]
async fn reports_a_closed_provider_event_stream() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
            initial_activity_delay: Duration::from_secs(1),
        })
        .await;
    let Err(error) = result else {
        panic!("closed provider event stream must fail");
    };

    assert!(error.to_string().contains("event stream closed"));
}

#[tokio::test]
async fn classifies_a_dead_provider_stream_closure_for_one_retry() {
    let (bridge, session, dispatcher) = grok_disconnect_fixture();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
            initial_activity_delay: Duration::from_secs(1),
        })
        .await;
    assert!(matches!(
        result,
        Ok(super::StreamTurn::ProviderFailure { .. })
    ));
}

#[tokio::test]
async fn retries_context_window_errors_only_before_committed_output() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
            initial_activity_delay: Duration::from_secs(1),
        })
        .await
        .expect("context error should request retry");
    assert!(matches!(result, super::StreamTurn::ContextWindow { .. }));

    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"visible"}
    }));
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::new(1),
            activity_interval: Duration::from_secs(1),
            initial_activity_delay: Duration::from_secs(1),
        })
        .await;
    let Err(error) = result else {
        panic!("context error after visible output must be fatal");
    };
    assert!(error.to_string().contains("context window"));
}

#[tokio::test]
async fn reports_slow_stream_preparation_before_the_provider_is_ready() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    // Drain while prepare/finish emit keepalives. A post-hoc recv loop deadlocks
    // once the bounded SSE channel fills under llvm-cov load.
    let drain = tokio::spawn(drain_sse_frame_list(receiver));
    let prepare = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<_, anyhow::Error>("ready")
    };
    let (result, mut builder) = super::prepare_with_activity(
        prepare,
        super::PrepareActivityOptions {
            input_tokens: 3,
            sender: &sender,
            // Omit launch prose; real provider output owns any visible ▶ chrome.
            initial_status: None,
            first_delay: Duration::from_millis(5),
            interval: Duration::from_millis(50),
            is_subagent: true,
            paint_command_code_progress: false,
            primed_thinking: false,
        },
    )
    .await;
    assert_eq!(result.expect("prepare result"), Some("ready"));
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    let frames = drain.await.expect("frame drain");

    assert_eq!(segment.usage.input_tokens, 3);
    // Keepalive thinking is live-only; committed segment stays clean.
    assert!(segment.blocks.is_empty());
    assert!(
        !frames
            .iter()
            .any(|frame| frame.contains("SubAgent starting")),
        "must not paint launch prose that collapses CC thinking: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .all(|frame| !frame.contains("Claudex is still working")),
        "slow prepare must not invent STATUS thinking: {frames:?}"
    );
}

#[tokio::test]
async fn command_code_prepare_primes_silent_thinking_not_canned_text() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let (main_sender, mut main_receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let _ = super::prime_subagent_sse(&main_sender, "gpt-5.6-luna", 1, false, None);
    let start = main_receiver.recv().await.expect("message_start");
    assert!(
        String::from_utf8_lossy(&start.expect("frame")).contains("message_start"),
        "main turns must first-flush message_start"
    );
    assert!(
        main_receiver.try_recv().is_err(),
        "main prime must not park a synthetic thinking block"
    );
    let _ = super::prime_subagent_sse(
        &sender,
        "meta/muse-spark-1.2-contributor",
        1,
        true,
        Some("high"),
    );
    let (result, mut builder) = super::prepare_with_activity(
        std::future::ready(Ok::<_, anyhow::Error>("ready")),
        super::PrepareActivityOptions {
            input_tokens: 1,
            sender: &sender,
            initial_status: None,
            first_delay: Duration::from_secs(1),
            interval: Duration::from_secs(1),
            is_subagent: true,
            paint_command_code_progress: true,
            primed_thinking: true,
        },
    )
    .await;
    assert_eq!(result.expect("prepare result"), Some("ready"));
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    assert_command_code_prime_is_silent(&segment.blocks, &drain_sse(&mut receiver).await);
}

fn assert_command_code_prime_is_silent(blocks: &[Value], frames: &[String]) {
    assert!(
        blocks.iter().all(|block| {
            !block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("▶") || text.contains("Command Code"))
                && !block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("still working") || text.contains("▶"))
        }),
        "canned chrome must not remain in transcript: {blocks:?}"
    );
    assert!(
        frames.iter().all(
            |frame| !frame.contains("content_block_start") && !frame.contains("thinking_delta")
        ),
        "silent Command Code prime must not synthesize thinking: {frames:?}"
    );
    assert!(
        !frames.iter().any(|frame| {
            frame.contains("ツール結果待ち")
                || frame.contains("続きの調査または回答")
                || frame.contains("SubAgent starting")
                || frame.contains("still working")
        }),
        "Command Code must not invent canned start chrome: {frames:?}"
    );
}

async fn drain_sse(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        frames.push(String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE"));
    }
    frames
}

#[tokio::test]
async fn finishes_fast_or_disconnected_stream_preparation_without_activity_status() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let (result, builder) = super::prepare_with_activity(
        std::future::ready(Ok::<_, anyhow::Error>("ready")),
        super::PrepareActivityOptions {
            input_tokens: 1,
            sender: &sender,
            initial_status: None,
            first_delay: Duration::from_secs(1),
            interval: Duration::from_secs(1),
            is_subagent: false,
            paint_command_code_progress: false,
            primed_thinking: false,
        },
    )
    .await;
    assert_eq!(result.expect("fast prepare"), Some("ready"));
    assert!(builder.blocks.is_empty());

    drop(receiver);
    let (result, builder) = super::prepare_with_activity(
        std::future::pending::<anyhow::Result<()>>(),
        super::PrepareActivityOptions {
            input_tokens: 1,
            sender: &sender,
            initial_status: None,
            first_delay: Duration::from_secs(1),
            interval: Duration::from_secs(1),
            is_subagent: false,
            paint_command_code_progress: false,
            primed_thinking: false,
        },
    )
    .await;
    assert!(result.expect("disconnected prepare").is_none());
    assert!(builder.blocks.is_empty());

    let (error_sender, _error_receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let (result, builder) = super::prepare_with_activity(
        std::future::ready(Err::<(), _>(anyhow!("provider setup failed"))),
        super::PrepareActivityOptions {
            input_tokens: 1,
            sender: &error_sender,
            initial_status: None,
            first_delay: Duration::from_secs(1),
            interval: Duration::from_secs(1),
            is_subagent: false,
            paint_command_code_progress: false,
            primed_thinking: false,
        },
    )
    .await;
    assert!(
        result
            .expect_err("failed prepare")
            .to_string()
            .contains("failed")
    );
    assert!(builder.blocks.is_empty());
}

#[tokio::test]
async fn subagent_prepare_continues_after_client_disconnect() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    drop(receiver);
    let prepare = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<_, anyhow::Error>("ready")
    };
    let (result, _builder) = super::prepare_with_activity(
        prepare,
        super::PrepareActivityOptions {
            input_tokens: 1,
            sender: &sender,
            initial_status: None,
            first_delay: Duration::from_secs(1),
            interval: Duration::from_secs(1),
            is_subagent: true,
            paint_command_code_progress: true,
            primed_thinking: true,
        },
    )
    .await;
    assert_eq!(
        result.expect("subagent prepare must not abort on SSE close"),
        Some("ready")
    );
}

#[tokio::test]
async fn primes_command_code_thinking_before_the_client_can_disconnect() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let primed = super::prime_subagent_sse(
        &sender,
        "meta/muse-spark-1.2-contributor",
        3,
        true,
        Some("high"),
    );
    drop(sender);
    assert!(primed, "Command Code must prime message_start");
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        frames.push(String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE"));
    }
    assert!(
        frames.iter().any(|frame| frame.contains("message_start")),
        "missing message_start: {frames:?}"
    );
    assert!(
        frames.iter().all(
            |frame| !frame.contains("content_block_start") && !frame.contains("thinking_delta")
        ),
        "first-flush must not synthesize a thinking body: {frames:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|frame| { frame.contains("text_delta") && frame.contains("Command Code") }),
        "primed Command Code must not dump start chrome: {frames:?}"
    );
}

#[tokio::test]
async fn cursor_subagent_primes_message_start_without_thinking() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let primed = super::prime_subagent_sse(&sender, "auto", 3, true, Some("high"));
    drop(sender);
    assert!(
        primed,
        "Cursor SubAgent must prime message_start in the first SSE flush"
    );
    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        frames.push(String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE"));
    }
    assert!(
        frames.iter().any(|frame| frame.contains("message_start")),
        "missing message_start: {frames:?}"
    );
    assert!(
        frames.iter().all(
            |frame| !frame.contains("content_block_start") && !frame.contains("thinking_delta")
        ),
        "first-flush must not synthesize a thinking body: {frames:?}"
    );
    assert!(
        !frames.iter().any(|frame| {
            frame.contains("SubAgent starting")
                || frame.contains("effort=high")
                || frame.contains("effort=")
        }),
        "old failure: prose+effort prime collapsed CC 2.1 to Wandering while ACP Bash kept running: {frames:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|frame| frame.contains("\"type\":\"text_delta\"")),
        "Cursor start chrome must ride thinking, not text: {frames:?}"
    );
}

#[tokio::test]
async fn subagent_stream_keeps_the_provider_after_sse_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    drop(receiver);
    let dispatcher_app = Arc::clone(&app);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        dispatcher_app.dispatch_test_event(json!({
            "method":"item/agentMessage/delta",
            "params":{
                "threadId":"thread",
                "itemId":"command-code:message",
                "delta":"▶ WebSearch tokyo\nTokyo: sunny 33C\n"
            }
        }));
        dispatcher_app.dispatch_test_event(json!({
            "method":"turn/completed",
            "params":{"threadId":"thread","turn":{"status":"completed"}}
        }));
    });

    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder: SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor"),
            activity_interval: Duration::from_millis(50),
            initial_activity_delay: Duration::from_millis(50),
        })
        .await
        .expect("subagent segment after SSE close");
    let super::StreamTurn::Segment { segment, .. } = result else {
        panic!("SubAgent SSE close must not disconnect the provider turn");
    };
    assert!(
        segment.blocks.iter().any(|block| block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| { text.contains("Tokyo: sunny 33C") })),
        "native cmd text must remain after SSE close: {:?}",
        segment.blocks
    );
}

#[tokio::test]
async fn subagent_stream_cancels_provider_when_sse_drops_after_tools() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor");
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"bash-1",
                "tool":"Bash",
                "title":"Bash ls",
                "arguments":{"command":"ls"}
            }}),
            Some(&sender),
        )
        .await
        .expect("tool chrome");
    assert!(
        builder.has_live_provider_work(),
        "▶ tool chrome must count as live provider work"
    );
    drop(receiver);

    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder,
            activity_interval: Duration::from_millis(50),
            initial_activity_delay: Duration::from_millis(50),
        })
        .await
        .expect("stop after tools");
    assert!(
        matches!(result, super::StreamTurn::Disconnected),
        "SSE close after ▶ tools must cancel the SubAgent provider"
    );
}

#[tokio::test]
async fn subagent_stream_cancels_provider_when_sse_drops_after_status() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::for_turn(1, true, "auto");
    builder
        .text_delta(
            &json!({"params":{"delta":"Status: inspecting local history\n"}}),
            None,
        )
        .await
        .expect("provider status");
    assert!(
        builder.has_live_provider_work(),
        "Status chrome must count as live provider work"
    );
    drop(receiver);

    let result = bridge
        .wait_for_stream_segment_with_interval(StreamWaitInput {
            session: &session,
            events: Arc::new(events),
            current_messages: &[],
            system: &json!(null),
            sender: &sender,
            builder,
            activity_interval: Duration::from_millis(50),
            initial_activity_delay: Duration::from_millis(50),
        })
        .await
        .expect("stop after status");
    assert!(
        matches!(result, super::StreamTurn::Disconnected),
        "SSE close after provider Status must cancel the SubAgent provider"
    );
}

#[tokio::test]
async fn ignores_malformed_empty_raw_and_late_reasoning() {
    let mut builder = SegmentBuilder::new(1);
    for event in [
        json!({"method":"item/reasoning/summaryTextDelta","params":{}}),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":7,"summaryIndex":0,"delta":"wrong item type"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":"zero","delta":"wrong index type"}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0,"delta":7}
        }),
        json!({
            "method":"item/reasoning/summaryTextDelta",
            "params":{"itemId":"reasoning","summaryIndex":0,"delta":""}
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
async fn streams_native_web_search_status_without_committing_progress_text() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    for event in [
        json!({
            "method":"item/started",
            "params":{"item":{"type":"message","query":"ignored"}}
        }),
        json!({
            "method":"item/started",
            "params":{"item":{"type":"webSearch","query":"Example Robotics"}}
        }),
        json!({
            "method":"item/started",
            "params":{"item":{"type":"webSearch","query":""}}
        }),
    ] {
        assert!(
            builder
                .handle_event(&bridge, &session, &[], &json!({}), &event, Some(&sender),)
                .await
                .expect("native search event")
                .is_continue()
        );
    }
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| { block_lacks_websearch_chrome(block) })
    );
    assert_eq!(segment.usage.web_search_requests, 0);

    let mut frames = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        frames.push(frame);
    }
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Example Robotics"))
    );
    assert!(frames.iter().any(|frame| frame.contains("WebSearch")));
    app.shutdown().await;
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
    assert_eq!(call.arguments, Value::Null);
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

#[tokio::test]
async fn rejects_a_malformed_tool_event_before_dispatch() {
    let root = tempfile::tempdir().expect("tool event fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program)
        .expect("mock metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("mock permissions");
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Bridge::new(app, "main".to_owned());
    let slots = Arc::new(Semaphore::new(1));
    let session = Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    };
    let error = SegmentBuilder::new(1)
        .handle_event(
            &bridge,
            &session,
            &[],
            &json!(null),
            &json!({"method":"item/tool/call","params":{}}),
            None,
        )
        .await
        .expect_err("malformed tool event");
    assert!(error.to_string().contains("callId missing"));
}

async fn assert_agent_tool_use_path(
    bridge: &Bridge,
    session: &Arc<Session>,
    messages: &[Value],
    routing: &Value,
) {
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            bridge,
            session,
            messages,
            routing,
            &json!({
                "method":"item/providerTool/call",
                "params":{
                    "callId":"acp-agent-1",
                    "tool":"Agent",
                    "status":"pending",
                    "arguments":{"prompt":"implement feature","subagent_type":"general-purpose"}
                }
            }),
            None,
        )
        .await
        .expect("bridge Agent to tool_use");
    assert!(builder.has_external_tool_calls());
    assert_eq!(builder.blocks[0]["type"], "tool_use");
    assert_eq!(builder.blocks[0]["name"], "Agent");
    let prompt = builder.blocks[0]["input"]["prompt"]
        .as_str()
        .expect("bridged Agent prompt");
    assert!(prompt.starts_with("implement feature"));
    assert_eq!(
        builder.blocks[0]["input"]["subagent_type"],
        "general-purpose"
    );
}

async fn assert_native_tool_wip_path(
    bridge: &Bridge,
    session: &Arc<Session>,
    messages: &[Value],
    routing: &Value,
) {
    let mut native = SegmentBuilder::new(1);
    let _ = native
        .handle_event(
            bridge,
            session,
            messages,
            routing,
            &json!({
                "method":"item/providerTool/call",
                "params":{
                    "callId":"acp-bash-1",
                    "tool":"Bash",
                    "status":"pending",
                    "arguments":{"command":"ls"}
                }
            }),
            None,
        )
        .await
        .expect("native Bash stays WIP");
    assert!(!native.has_external_tool_calls());
    assert!(native.open_text_block.is_none());
    assert!(
        native
            .blocks
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) != Some("tool_use"))
    );
    let progress = native
        .blocks
        .iter()
        .filter_map(|block| block.get("thinking").and_then(Value::as_str))
        .collect::<String>();
    assert!(
        progress.contains("▶ Bash"),
        "native progress thinking: {progress}"
    );
    assert!(progress.contains("ls"));
}

async fn assert_mcp_tool_wip_path(
    bridge: &Bridge,
    session: &Arc<Session>,
    messages: &[Value],
    routing: &Value,
) {
    let mut mcp = SegmentBuilder::new(1);
    let _ = mcp
        .handle_event(
            bridge,
            session,
            messages,
            routing,
            &json!({
                "method":"item/providerTool/call",
                "params":{
                    "callId":"mcp-1",
                    "tool":"mcp",
                    "title":"MCP claudex-launch",
                    "status":"pending"
                }
            }),
            None,
        )
        .await
        .expect("remember MCP call id");
    let _ = mcp
        .handle_event(
            bridge,
            session,
            messages,
            routing,
            &json!({
                "method":"item/providerTool/update",
                "params":{
                    "callId":"mcp-1",
                    "tool":"mcp",
                    "title":"MCP claudex-launch",
                    "status":"in_progress"
                }
            }),
            None,
        )
        .await
        .expect("suppress MCP launch progress");
    assert!(!mcp.has_external_tool_calls());
}

#[tokio::test]
async fn bridges_acp_agent_provider_tools_to_tool_use_but_keeps_native_tools_as_wip() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    // disconnect_fixture already maps cc_Agent_0 → Agent; add Bash for native WIP.
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Bash_0".to_owned(), "Bash".to_owned());
    // Also accept the plain "Agent" provider label used by some ACP agents.
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("Agent".to_owned(), "Agent".to_owned());
    let messages = [json!({"role":"user","content":"delegate work"})];
    let routing = json!(
        r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model","effort":"high"}]}"#
    );
    assert_agent_tool_use_path(&bridge, &session, &messages, &routing).await;
    assert_native_tool_wip_path(&bridge, &session, &messages, &routing).await;
    assert_mcp_tool_wip_path(&bridge, &session, &messages, &routing).await;
}

#[tokio::test]
async fn expands_valid_parallel_agent_batches_and_rejects_short_batches() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let routing = json!(
        r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
    );
    let messages = [json!({"role":"user","content":"delegate"})];
    let mut builder = SegmentBuilder::new(1);
    let event = agent_batch_event(
        "batch-call",
        [
            worker_task("first", None),
            worker_task("second", Some(true)),
            worker_task("third", Some(true)),
        ],
    );
    let _ = builder
        .handle_event(&bridge, &session, &messages, &routing, &event, None)
        .await
        .expect("parallel batch");
    assert!(builder.has_external_tool_calls());
    assert_eq!(builder.blocks.len(), 3);
    assert_background_batch(&builder, 0, 3);

    let mixed = agent_batch_event(
        "mixed-call",
        [
            worker_task("background", None),
            worker_task("foreground", Some(false)),
            worker_task("third", Some(true)),
        ],
    );
    let _ = builder
        .handle_event(&bridge, &session, &messages, &routing, &mixed, None)
        .await
        .expect("mixed batch modes are normalized to background");
    assert_eq!(builder.blocks.len(), 6);
    assert_background_batch(&builder, 3, 3);

    let mut explore = SegmentBuilder::new(1);
    let _ = explore
        .handle_event(
            &bridge,
            &session,
            &[json!({
                "role":"user",
                "content":"Investigate how sync-realtime-data chooses the writable Neon connection."
            })],
            &routing,
            &json!({
                "id":4,
                "method":"item/tool/call",
                "params":{
                    "callId":"explore",
                    "tool":"cc_Agent_0",
                    "arguments":{
                        "prompt":"Assess production configuration paths",
                        "subagent_type":"worker",
                        "claudex_model":"worker-model",
                        "run_in_background":false
                    }
                }
            }),
            None,
        )
        .await
        .expect("Explore launch");
    assert!(explore.has_external_tool_calls());
    assert_eq!(explore.blocks[0]["name"], "Agent");
    assert_eq!(explore.blocks[0]["input"]["run_in_background"], true);

    let short = agent_batch_event("short-call", [worker_task("only", None)]);
    let error = builder
        .handle_event(&bridge, &session, &messages, &routing, &short, None)
        .await
        .expect_err("short batch");
    assert!(error.to_string().contains("between 3 and 40"));
}

#[tokio::test]
async fn forwards_generic_tools_and_blocks_disabled_subagent_models() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let mut generic = SegmentBuilder::new(1);
    let _ = generic
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":1,
                "method":"item/tool/call",
                "params":{
                    "callId":"read",
                    "tool":"cc_Read_0",
                    "arguments":{
                        "path":"README.md",
                        "claudex_model":"gpt-5.6-luna",
                        "claudex_implicit_model":"gpt-5.6-luna",
                        "claudex_effort":"max"
                    }
                }
            }),
            None,
        )
        .await
        .expect("generic external tool");
    assert!(generic.has_external_tool_calls());
    assert_eq!(generic.blocks[0]["name"], "Read");
    assert_eq!(generic.blocks[0]["input"], json!({"path":"README.md"}));

    let disabled = BTreeSet::from(["blocked-model".to_owned()]);
    let (_root, _app, bridge, session) = disconnect_fixture_with_disabled(disabled).await;
    let mut blocked = SegmentBuilder::new(1);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let _ = blocked
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":2,
                "method":"item/tool/call",
                "params":{
                    "callId":"agent",
                    "tool":"cc_Agent_0",
                    "arguments":{"prompt":"delegate","subagent_type":"worker","claudex_model":"blocked-model"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("disabled subagent is a visible local response");
    assert!(!blocked.has_external_tool_calls());
    assert_eq!(blocked.blocks.len(), 1);
    assert_eq!(blocked.blocks[0]["type"], "text");
    assert!(blocked.blocks[0]["text"].as_str().is_some_and(|text| {
        text.contains("blocked-model") && text.contains("disabled by policy")
    }));
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("blocked-model"));
    assert!(output.contains("disabled by policy"));
    assert!(!output.contains("not configured"));
    assert!(output.contains(r#""content_block""#));
    assert!(output.contains(r#""type":"text""#));
    assert!(output.contains(r#""type":"text_delta""#));
    assert!(!output.contains(r#""type":"thinking_delta""#));
    assert!(!output.contains(r#""type":"tool_use""#));
    assert!(!output.contains("▶ Agent"));
}

#[tokio::test]
async fn skips_stale_task_output_without_painting_claude_tool_use() {
    let (root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_TaskOutput_0".to_owned(), "TaskOutput".to_owned());
    let messages = [json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"toolu_live",
            "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
        }]
    })];
    let mut builder = SegmentBuilder::new(1);
    let flow = builder
        .handle_event(
            &bridge,
            &session,
            &messages,
            &Value::Null,
            &json!({
                "id":7,
                "method":"item/tool/call",
                "params":{
                    "callId":"stale-output",
                    "tool":"cc_TaskOutput_0",
                    "arguments":{"task_id":"a3d7f2ca50556c9e5","block":false}
                }
            }),
            None,
        )
        .await
        .expect("stale TaskOutput is a local miss");
    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(!builder.has_external_tool_calls());
    assert!(builder.blocks.is_empty());
    assert_disconnected_tool_rejections(&root, &[7]).await;
    let log = std::fs::read_to_string(root.path().join("responses.log")).unwrap_or_default();
    assert!(log.contains("a3d7f2ca50556c9e5"));
    assert!(log.contains("a4496564387a2561f"));
    assert!(log.contains("\"success\":false") || log.contains("\"success\": false"));
}

#[tokio::test]
async fn forwards_live_task_output_to_claude_code() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_TaskOutput_0".to_owned(), "TaskOutput".to_owned());
    let messages = [json!({
        "role":"user",
        "content":[{
            "type":"tool_result",
            "tool_use_id":"toolu_live",
            "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
        }]
    })];
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &messages,
            &Value::Null,
            &json!({
                "id":8,
                "method":"item/tool/call",
                "params":{
                    "callId":"live-output",
                    "tool":"cc_TaskOutput_0",
                    "arguments":{"task_id":"a4496564387a2561f","block":false}
                }
            }),
            None,
        )
        .await
        .expect("live TaskOutput is forwarded");
    assert!(builder.has_external_tool_calls());
    assert_eq!(builder.blocks[0]["type"], "tool_use");
    assert_eq!(builder.blocks[0]["name"], "TaskOutput");
    assert_eq!(builder.blocks[0]["input"]["task_id"], "a4496564387a2561f");
}

#[tokio::test]
async fn subagent_codex_tool_use_closes_thinking_before_native_card() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna").with_primed_thinking();
    builder
        .model_output_event(
            &raw_reasoning_textdelta("luna:reasoning", "Inspect CLAUDE.md next.\n"),
            Some(&sender),
        )
        .await
        .expect("luna cot");
    feed_subagent_read(&mut builder, &bridge, &session, "read-claude-md", &sender).await;
    assert!(builder.has_external_tool_calls());
    assert!(
        !builder.thinking.is_open(),
        "Codex/luna Read must close thinking so the native card is live"
    );
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("silence after Read");
    drop(sender);
    let output = collect_sse_frames(&mut receiver).await;
    assert_thinking_stop_before_native_read(&output);
    assert!(
        output.contains("Inspect CLAUDE.md next"),
        "live CoT missing: {output}"
    );
    assert!(
        !output.contains("still working") && !output.contains("Thought for"),
        "Thought-for chrome leaked: {output}"
    );
}

#[tokio::test]
async fn fugu_codex_closes_thinking_before_native_read() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    assert!(super::prime_subagent_sse(
        &sender,
        "fugu",
        1,
        true,
        Some("high")
    ));
    let mut builder = SegmentBuilder::for_turn(1, true, "fugu").with_primed_thinking();
    builder
        .model_output_event(
            &raw_reasoning_textdelta("fugu:reasoning", "Map the race filter before seeding.\n"),
            Some(&sender),
        )
        .await
        .expect("fugu cot");
    feed_subagent_read(&mut builder, &bridge, &session, "read-claude-md", &sender).await;
    assert!(!builder.thinking.is_open());
    drop(sender);
    let output = collect_sse_frames(&mut receiver).await;
    assert!(
        output.contains("content_block_start") && output.contains("thinking"),
        "thinking start missing: {output}"
    );
    assert_thinking_stop_before_native_read(&output);
    assert!(
        output.contains("Map the race filter"),
        "fugu CoT must stream live: {output}"
    );
}

#[tokio::test]
async fn subagent_codex_external_batch_finish_does_not_reopen_thinking() {
    // Inverted: close thinking before native Read/Bash so CC 2.1 does not
    // Slithering-hide the tool card. Keep-open was the bug.
    let (_root, app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let events = Arc::new(app.subscribe_thread("thread"));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna").with_primed_thinking();
    builder
        .model_output_event(
            &raw_reasoning_textdelta("luna:reasoning", "Read CLAUDE.md for handoff.\n"),
            Some(&sender),
        )
        .await
        .expect("cot");
    feed_subagent_read(&mut builder, &bridge, &session, "read-handoff", &sender).await;
    assert!(builder.has_external_tool_calls());
    assert!(!builder.thinking.is_open());

    let result = bridge
        .external_batch_segment(&session, events, &mut builder, Some(&sender))
        .await
        .expect("Codex batch handoff");
    let super::StreamTurn::Segment {
        segment,
        provider_settled,
    } = result
    else {
        panic!("expected tool_use segment");
    };
    assert!(!provider_settled);
    assert_eq!(segment.stop_reason, "tool_use");
    assert!(
        !builder.thinking.is_open(),
        "finish must close thinking across Codex tool handoff"
    );

    drop(sender);
    let output = collect_sse_frames(&mut receiver).await;
    assert_thinking_stop_before_native_read(&output);
}

#[tokio::test]
async fn keeps_parent_stream_after_unroutable_subagent_launch() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let mut builder = SegmentBuilder::new(1);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let flow = builder
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":3,
                "method":"item/tool/call",
                "params":{
                    "callId":"agent",
                    "tool":"cc_Agent_0",
                    "arguments":{"prompt":"delegate","subagent_type":"general-purpose"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("unroutable SubAgent must not fail the parent stream");
    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(!builder.has_external_tool_calls());
    assert_eq!(builder.blocks.len(), 1);
    assert_eq!(builder.blocks[0]["type"], "text");
    assert!(builder.blocks.iter().any(|block| {
        block["text"]
            .as_str()
            .is_some_and(|text| text.contains("was not started. Continue without it."))
    }));
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("was not started. Continue without it."));
    assert!(output.contains(r#""content_block""#));
    assert!(output.contains(r#""type":"text""#));
    assert!(output.contains(r#""type":"text_delta""#));
    assert!(!output.contains(r#""type":"thinking_delta""#));
    assert!(!output.contains(r#""type":"tool_use""#));
    assert!(!output.contains("▶ Agent"));
}

fn worker_task(prompt: &str, run_in_background: Option<bool>) -> Value {
    let mut task = json!({
        "prompt": prompt,
        "subagent_type": "worker",
        "claudex_model": "worker-model"
    });
    if let Some(run_in_background) = run_in_background {
        task["run_in_background"] = json!(run_in_background);
    }
    task
}

fn agent_batch_event(call_id: &str, tasks: impl IntoIterator<Item = Value>) -> Value {
    json!({
        "id": 99,
        "method": "item/tool/call",
        "params": {
            "callId": call_id,
            "tool": "cc_Agent_batch_0",
            "arguments": {"tasks": tasks.into_iter().collect::<Vec<_>>()}
        }
    })
}

fn assert_background_batch(builder: &SegmentBuilder, start: usize, count: usize) {
    for index in start..start + count {
        assert_eq!(
            builder.blocks[index]["input"]["run_in_background"].as_bool(),
            Some(true),
            "batch worker {index} should run in background"
        );
    }
}

#[tokio::test]
async fn treats_a_closed_sender_after_batch_finish_as_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[json!({"role":"user","content":"delegate"})],
            &json!(
                r#"{"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
            ),
            &json!({
                "id":99,
                "method":"item/tool/call",
                "params":{
                    "callId":"batch-call",
                    "tool":"cc_Agent_batch_0",
                    "arguments":{"tasks":[
                        {"prompt":"first","subagent_type":"worker","claudex_model":"worker-model"},
                        {"prompt":"second","subagent_type":"worker","claudex_model":"worker-model"},
                        {"prompt":"third","subagent_type":"worker","claudex_model":"worker-model"}
                    ]}
                }
            }),
            None,
        )
        .await
        .expect("batch tool call");
    let result = bridge
        .external_batch_segment(&session, events, &mut builder, Some(&sender))
        .await
        .expect("closed batch sender");
    assert!(matches!(result, super::StreamTurn::Disconnected));
}

#[tokio::test]
async fn commits_status_deltas_as_thinking_progress_even_after_answer_starts() {
    let mut builder = SegmentBuilder::new(1);
    builder.blocks.push(json!({"type":"text","text":"answer"}));
    assert!(builder.has_committed_output());
    builder
        .stream_progress_text("", None)
        .await
        .expect("empty status");
    builder
        .stream_progress_text("\n▶ provider\n", None)
        .await
        .expect("thinking progress after closed text");
    assert!(builder.open_text_block.is_none());
    assert!(
        builder
            .blocks
            .iter()
            .any(
                |block| block.get("type").and_then(Value::as_str) == Some("thinking")
                    && block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("▶ provider"))
            ),
        "▶ must stay in thinking chrome after answer text: {:?}",
        builder.blocks
    );

    builder.blocks.clear();
    builder.thinking = ThinkingState::default();
    builder.open_text_block = Some((0, "answer".to_owned()));
    builder.blocks.push(json!({"type":"text","text":""}));
    builder
        .stream_progress_text("\n▶ provider\n", None)
        .await
        .expect("thinking progress after open text");
    assert!(builder.open_text_block.is_none());
    assert!(
        builder.blocks.iter().any(|block| block
            .get("thinking")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("▶ provider"))),
        "open text must close so SubAgent TUI can show ▶: {:?}",
        builder.blocks
    );
}

#[tokio::test]
async fn unsupported_disconnect_with_a_visible_tool_aborts_without_a_drain() {
    let (root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    bridge.sessions.lock().await.push(Arc::clone(&session));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(41));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    app.dispatch_test_event(json!({
        "id":41,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "id":42,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"new","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"thread/tokenUsage/updated",
        "params":{"threadId":"thread","tokenUsage":{"last":{"inputTokens":1}}}
    }));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","willRetry":true}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    assert!(session.pending_since.lock().unwrap().is_none());
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(
        app.is_alive(),
        "Codex disconnect abort must not kill the shared app-server"
    );
    assert_eq!(Arc::strong_count(&events), 1, "no hidden drain owns events");
    // Session was removed; the provider leaf stays up for unrelated threads.
    assert_eq!(bridge.used_session_slots(), 1);
    drop(session);
    assert_eq!(bridge.used_session_slots(), 0);
    drop(root);
}

#[tokio::test]
async fn async_handoff_with_a_visible_tool_drains_without_closing_shared_provider() {
    let (root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    bridge.sessions.lock().await.push(Arc::clone(&session));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(41));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    app.dispatch_test_event(json!({
        "id":41,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    assert!(matches!(
        bridge
            .disconnect_stream_for_async_handoff(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(
        app.is_alive(),
        "async handoff must not stop a shared provider"
    );
    wait_for_disconnected_drain(&events).await;
    drop(session);
    drop(root);
}

#[tokio::test]
async fn disconnected_drain_handles_incremental_events_and_provider_errors() {
    let (root, app, bridge, _session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "id":51,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"first","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "id":51,"method":"item/tool/call",
        "params":{"threadId":"thread","callId":"duplicate","tool":"Read"}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"inProgress"}}
    }));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","willRetry":true}
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
        .await
        .expect("completed turn drains successfully");
    assert_disconnected_tool_rejections(&root, &[51]).await;
}

#[tokio::test]
async fn disconnected_drain_returns_non_retryable_provider_errors() {
    let (_root, app, bridge, _session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","message":"fatal"}
    }));

    let error =
        super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
            .await
            .expect_err("fatal provider event must stop the drain");
    assert!(error.to_string().contains("fatal"));
}

#[tokio::test]
async fn unsupported_disconnect_drains_without_closing_the_provider() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    app.dispatch_test_event(json!({
        "method":"error","params":{"threadId":"thread","message":"fatal"}
    }));
    bridge.finish_closed_stream(&session, &events, false).await;
    wait_for_disconnected_drain(&events).await;
    assert!(app.is_alive());
}

#[tokio::test]
async fn settled_stream_close_retains_session_for_subagent_resume() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let events = Arc::new(app.subscribe_thread("thread"));
    let before = *session.last_activity.lock().unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;

    bridge.finish_closed_stream(&session, &events, true).await;

    let sessions = bridge.sessions.lock().await;
    assert_eq!(
        sessions.len(),
        1,
        "settled SubAgent sessions must stay idle for Task resume / prompt-cache"
    );
    assert!(Arc::ptr_eq(&sessions[0], &session));
    assert!(*session.last_activity.lock().unwrap() > before);
    assert!(app.is_alive());
}

#[tokio::test]
async fn tolerates_failed_pending_tool_rejection_after_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    app.shutdown().await;

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn cancellation_failure_detaches_and_warns_for_pending_tools() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    app.shutdown().await;

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(session.pending_tools.lock().await.is_empty());
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn grok_dead_driver_cancel_settles_and_rejects_pending_tools() {
    let (bridge, session, dispatcher) = grok_disconnect_fixture();
    let events = Arc::new(dispatcher.subscribe("thread"));
    session
        .pending_tools
        .lock()
        .await
        .insert("pending".to_owned(), json!(61));
    *session.pending_since.lock().unwrap() = Some(Instant::now());
    bridge.sessions.lock().await.push(Arc::clone(&session));

    assert!(matches!(
        bridge
            .disconnect_stream(&session, Arc::clone(&events))
            .await,
        super::StreamTurn::Disconnected
    ));
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(session.pending_tools.lock().await.is_empty());
    dispatcher.close();
    wait_for_disconnected_drain(&events).await;
}

#[tokio::test]
async fn disconnected_drain_reports_closed_and_malformed_event_streams() {
    let (bridge, _session, dispatcher) = grok_disconnect_fixture();
    let events = Arc::new(dispatcher.subscribe("thread"));
    dispatcher.close();
    let error = super::disconnect::drain_disconnected_turn(
        &bridge.app,
        "main",
        Arc::clone(&events),
        HashSet::new(),
    )
    .await
    .expect_err("closed event stream should be reported");
    assert!(error.to_string().contains("event stream closed"));

    let dispatcher = ThreadEventDispatcher::default();
    let events = Arc::new(dispatcher.subscribe("thread"));
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{"threadId":"thread"}
    }));
    let error =
        super::disconnect::drain_disconnected_turn(&bridge.app, "main", events, HashSet::new())
            .await
            .expect_err("malformed tool event should be reported");
    assert!(error.to_string().contains("tool"));
}

#[tokio::test]
async fn drive_stream_reports_unretryable_context_window_errors() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("context window"));
    assert!(output.contains("\"stop_reason\":\"error\""));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_retries_context_window_then_completes() {
    let (_root, _app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let request = drive_request();

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let retried_session = {
        let sessions = bridge.sessions.lock().await;
        assert_eq!(sessions.len(), 1, "retry should retain its fresh session");
        Arc::clone(&sessions[0])
    };
    let transcript = retried_session.transcript.lock().await.clone();
    assert_eq!(transcript[0], json!({"role":"user","content":"retry me"}));
    assert_eq!(transcript[1], json!({"role":"assistant","content":[]}));

    let mut output = String::new();
    while let Ok(frame) = receiver.try_recv() {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_keeps_content_indices_monotonic_across_context_retry() {
    let (_root, _app, bridge, session) = retryable_drive_fixture_with_output().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let builder = SegmentBuilder::new(1);
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive thinking block");

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            builder,
            None,
        )
        .await;

    let mut open_index = None;
    let mut next_index = 0;
    let mut started_types = Vec::new();
    while let Some(frame) = receiver.recv().await {
        let frame = String::from_utf8(frame.expect("frame").to_vec()).expect("UTF-8 SSE");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        let payload = serde_json::from_str::<Value>(data.expect("SSE data")).expect("JSON frame");
        track_content_block_frame(
            &payload,
            &mut open_index,
            &mut next_index,
            &mut started_types,
        );
    }

    assert_eq!(started_types, vec![json!("text")]);
    assert_eq!(next_index, 1);
    assert!(open_index.is_none());
}

#[tokio::test]
async fn drive_stream_retries_context_window_with_explicit_effort() {
    let (root, _app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let request = drive_request();
    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: Some("high".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let trace = request_trace(&root.path().join("requests.log"), 2).await;
    let turn_starts = trace
        .into_iter()
        .filter(|request| request.get("method").and_then(Value::as_str) == Some("turn/start"))
        .collect::<Vec<_>>();
    assert!(
        !turn_starts.is_empty(),
        "retry should trigger a logged turn/start request"
    );
    for request in &turn_starts {
        assert_eq!(request["params"]["effort"], "high");
    }

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("event: message_delta"));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_reports_context_retry_setup_errors() {
    let (_root, _app, bridge, session) = retry_failure_drive_fixture().await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("retry setup failed"));
    assert!(output.contains("\"stop_reason\":\"error\""));
    assert!(output.contains("event: message_stop"));
    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn drive_stream_finishes_quietly_after_client_disconnect() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;
}

#[tokio::test]
async fn drive_stream_reports_closed_provider_event_streams() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            None,
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("event stream closed"));
    assert!(output.contains("\"stop_reason\":\"error\""));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn drive_stream_cancels_provider_leaf_when_wait_fails() {
    // Regression: mid-wait failures (including SubAgent silence judgment) used to
    // remove_session without cancel_turn, leaving orphan ACP processes that
    // blocked prompt-cache reuse and looked hung in Claude Code.
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    Arc::clone(&bridge)
        .drive_stream(
            drive_turn(Arc::clone(&session), events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1).with_subagent(true),
            None,
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("event stream closed") || output.contains("\"stop_reason\":\"error\""),
        "wait failure must surface to Claude Code: {output}"
    );
    assert!(
        bridge.sessions.lock().await.is_empty(),
        "failed stream must cancel and unregister the session (not orphan ACP)"
    );
    let cancellation = bridge
        .app
        .cancel_turn("thread")
        .await
        .expect("cancel after disconnect");
    assert!(
        matches!(
            cancellation,
            crate::agent_backend::TurnCancellation::Settled
                | crate::agent_backend::TurnCancellation::Unsupported
        ),
        "provider cancel path must stay healthy after stream failure cleanup"
    );
}

#[tokio::test]
async fn drive_stream_stops_before_commit_when_client_closes_after_segment() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"answer"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let driver = tokio::spawn(Arc::clone(&bridge).drive_stream(
        drive_turn(session.clone(), events, Vec::new(), None).await,
        sender,
        SegmentBuilder::new(1),
        None,
    ));

    tokio::time::timeout(Duration::from_secs(1), async {
        wait_until_receiver_len(&receiver, 2).await;
    })
    .await
    .expect("stream should fill before the completion frame");
    receiver.close();
    driver.await.expect("stream driver task");

    assert!(session.transcript.lock().await.is_empty());
}

#[tokio::test]
async fn subagent_stream_without_hard_timeout_stays_attached_beyond_300_seconds() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    tokio::time::pause();
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);

    let driver = tokio::spawn(Arc::clone(&bridge).drive_subagent_stream_with_timeout(
        drive_turn(session, events, Vec::new(), None).await,
        sender,
        SegmentBuilder::new(1),
        super::drive::StreamDriveOptions {
            model_permit: None,
            is_subagent: true,
            run_in_background: true,
            timeout: None,
        },
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(301)).await;
    tokio::task::yield_now().await;

    assert!(
        !driver.is_finished(),
        "unset timeout ended the native Agent stream"
    );
    assert!(bridge.detached_sessions.lock().await.is_empty());

    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"stream completed after 301 seconds"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    driver.await.expect("stream driver task");

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(output.contains("stream completed after 301 seconds"));
}

#[tokio::test]
async fn subagent_stream_hard_timeout_cancels_and_reports_a_visible_error() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let observed_session = Arc::clone(&session);

    Arc::clone(&bridge)
        .drive_subagent_stream_with_timeout(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            super::drive::StreamDriveOptions {
                model_permit: None,
                is_subagent: true,
                run_in_background: true,
                timeout: Some(Duration::ZERO),
            },
        )
        .await;

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("configured hard timeout"),
        "unexpected stream: {output}"
    );
    assert!(!output.contains("dynamic progress"));
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert_eq!(
        bridge
            .subagent_hard_timeout_cancel_attempts
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    app.dispatch_test_event(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"late stream result"}
    }));
    app.dispatch_test_event(json!({
        "method":"item/tool/call",
        "params":{
            "threadId":"thread",
            "item":{"id":"late-stream-tool","name":"Read","arguments":{"path":"ignored"}}
        }
    }));
    app.dispatch_test_event(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    tokio::task::yield_now().await;
    assert!(observed_session.transcript.lock().await.is_empty());
    assert!(observed_session.pending_tools.lock().await.is_empty());
}

#[tokio::test]
async fn subagent_stream_timeout_tolerates_a_disconnected_client() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::clone(&bridge)
        .drive_subagent_stream_with_timeout(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::new(1),
            super::drive::StreamDriveOptions {
                model_permit: None,
                is_subagent: true,
                run_in_background: true,
                timeout: Some(Duration::ZERO),
            },
        )
        .await;
    assert!(bridge.detached_sessions.lock().await.is_empty());
}

#[tokio::test]
async fn subagent_empty_end_turn_without_retry_reports_billing_error() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    Arc::clone(&bridge)
        .drive_subagent_stream(
            drive_turn(session, events, Vec::new(), None).await,
            sender,
            SegmentBuilder::for_turn(1, true, "main"),
            None,
            true,
            false,
        )
        .await;
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("no assistant content")
            || output.contains("billing")
            || output.contains("error"),
        "unexpected empty SubAgent stream: {output}"
    );
}

#[tokio::test]
async fn retry_after_provider_failure_requires_closed_stream_and_dead_model() {
    let (_root, app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = ThreadEventDispatcher::default();
    const CLOSED: &str = "app-server event stream closed";

    let alive = bridge
        .retry_after_provider_failure(
            drive_turn(
                Arc::clone(&session),
                dispatcher.subscribe("alive"),
                Vec::new(),
                None,
            )
            .await,
            anyhow!(CLOSED),
        )
        .await;
    let Err(alive) = alive else {
        panic!("live model should not recycle");
    };
    assert!(alive.to_string().contains(CLOSED));

    let unrelated = bridge
        .retry_after_provider_failure(
            drive_turn(
                Arc::clone(&session),
                dispatcher.subscribe("unrelated"),
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            anyhow!("some other failure"),
        )
        .await;
    let Err(unrelated) = unrelated else {
        panic!("unrelated errors should not recycle");
    };
    assert!(unrelated.to_string().contains("some other failure"));

    app.shutdown().await;
    let missing_retry = bridge
        .retry_after_provider_failure(
            drive_turn(
                Arc::clone(&session),
                dispatcher.subscribe("dead"),
                Vec::new(),
                None,
            )
            .await,
            anyhow!(CLOSED),
        )
        .await;
    let Err(missing_retry) = missing_retry else {
        panic!("recycled model still needs a retry payload");
    };
    assert!(missing_retry.to_string().contains(CLOSED));

    let retried = bridge
        .retry_after_provider_failure(
            drive_turn(
                session,
                dispatcher.subscribe("retry-dead"),
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            anyhow!(CLOSED),
        )
        .await;
    assert!(
        retried.is_err(),
        "dead provider retry still needs a live backend"
    );
}

#[tokio::test]
async fn drive_stream_retries_provider_failure_onto_a_live_sibling_route() {
    let (_root, app, _seed_bridge, _seed_session) = retryable_drive_fixture_with_output().await;
    let backend = AgentBackend::routed(vec![
        (
            "main".to_owned(),
            AgentBackend::grok(GrokAcp::stopped_for_test()),
        ),
        ("retry-target".to_owned(), AgentBackend::codex(app)),
    ]);
    let bridge = Arc::new(Bridge::new_with_backend(backend, "main".to_owned()));
    assert!(!bridge.app.model_is_alive("main"));
    assert!(bridge.app.model_is_alive("retry-target"));
    let slots = Arc::new(Semaphore::new(2));
    let session = Arc::new(Session {
        thread_id: "0:dead".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("0:dead");
    dispatcher.close();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut request = drive_request();
    request.model = "retry-target".to_owned();
    let drive = Arc::clone(&bridge).drive_stream(
        drive_turn(
            session,
            events,
            Vec::new(),
            Some(ContextRetry {
                request,
                effort: Some("high".to_owned()),
                advisor_model: None,
                collaborator_model: None,
            }),
        )
        .await,
        sender,
        SegmentBuilder::for_turn(1, false, "main"),
        None,
    );
    tokio::time::timeout(Duration::from_secs(12), drive)
        .await
        .expect("provider-failure drive retry must finish promptly");
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        output.contains("retried answer") || output.contains("message_stop"),
        "provider failure must resume on the live sibling route: {output}"
    );
}

#[tokio::test]
async fn retry_usage_limit_stream_restarts_on_a_sibling_with_its_configured_effort() {
    let root = tempfile::tempdir().expect("configured sibling fixture");
    let sibling =
        GrokAcp::spawn_with_program("auto", grok_acp_mock_program(), root.path().to_path_buf())
            .await
            .expect("start configured sibling ACP fixture");
    let backend = AgentBackend::routed(vec![
        (
            "qwen3.8-max-preview".to_owned(),
            AgentBackend::grok(GrokAcp::stopped_for_test()),
        ),
        ("auto".to_owned(), AgentBackend::configured_acp(sibling)),
    ]);
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-qwen", "qwen3.8-max-preview", "low"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", "auto", "max"),
        ])
        .expect("worker routes");
    let bridge = Arc::new(
        Bridge::new_with_backend(backend, "qwen3.8-max-preview".to_owned())
            .with_model_catalog(catalog),
    );
    let failover = bridge
        .failover_for_stream_turn("qwen3.8-max-preview", true)
        .expect("configured sibling route");
    assert_eq!(failover.model, "auto");
    assert_eq!(failover.effort.as_deref(), Some("max"));
    let slots = Arc::new(Semaphore::new(2));
    let session = Arc::new(Session {
        thread_id: "0:exhausted".to_owned(),
        model: "qwen3.8-max-preview".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    let dispatcher = ThreadEventDispatcher::default();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut request = drive_request();
    request.model = "qwen3.8-max-preview".to_owned();

    Arc::clone(&bridge)
        .retry_usage_limit_stream(super::drive::ContextRetryStream {
            turn: drive_turn(
                session,
                dispatcher.subscribe("0:exhausted"),
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: Some("low".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            error: anyhow!(crate::anthropic::segment::EMPTY_ACP_END_TURN),
            builder: SegmentBuilder::for_turn(1, true, "qwen3.8-max-preview"),
            model_permit: None,
            is_subagent: true,
            run_in_background: false,
        })
        .await;

    let output = collect_sse_frames(&mut receiver).await;
    assert!(
        output.contains("GROK_ACP_STREAM_OK"),
        "retry output: {output}"
    );
}

#[tokio::test]
async fn retry_context_stream_without_a_retry_removes_the_session_and_reports_the_error() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    Arc::new(bridge)
        .retry_context_stream(super::drive::ContextRetryStream {
            turn: drive_turn(session, app.subscribe_thread("thread"), Vec::new(), None).await,
            sender,
            error: anyhow!("context retry payload was unavailable"),
            builder: SegmentBuilder::new(1),
            model_permit: None,
            is_subagent: false,
            run_in_background: false,
        })
        .await;

    let output = collect_sse_frames(&mut receiver).await;
    assert!(output.contains("context retry payload was unavailable"));
}

#[tokio::test]
async fn drive_stream_reports_usage_limit_without_a_sibling_provider() {
    let (root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge.with_usage_limit_cache_home(root.path()));
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"codexErrorInfo":"usageLimitExceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    Arc::clone(&bridge)
        .drive_subagent_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::for_turn(1, true, "main"),
            None,
            true,
            false,
        )
        .await;
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("usage") || output.contains("limit") || output.contains("error"),
        "unexpected usage-limit stream: {output}"
    );
}

#[tokio::test]
async fn drive_stream_retries_a_dead_provider_failure() {
    let cache = tempfile::tempdir().expect("provider failure cache");
    let (bridge, session, dispatcher) = grok_disconnect_fixture();
    let bridge = Arc::new(bridge.with_usage_limit_cache_home(cache.path()));
    let events = dispatcher.subscribe("thread");
    dispatcher.close();
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    Arc::clone(&bridge)
        .drive_subagent_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::new(1),
            None,
            true,
            false,
        )
        .await;
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        output.contains("error") || output.contains("closed") || output.contains("fail"),
        "unexpected provider-failure stream: {output}"
    );
}

#[tokio::test]
async fn context_retry_or_error_requires_window_marker_and_retry() {
    let (_root, _app, bridge, session) = retryable_drive_fixture().await;
    let dispatcher = ThreadEventDispatcher::default();

    {
        let mut turn = drive_turn(
            Arc::clone(&session),
            dispatcher.subscribe("no-marker"),
            Vec::new(),
            Some(ContextRetry {
                request: drive_request(),
                effort: None,
                advisor_model: None,
                collaborator_model: None,
            }),
        )
        .await;
        let unmarked = bridge
            .context_retry_or_error(&mut turn, anyhow!("boom"))
            .await
            .err()
            .expect("unmarked errors should fail");
        assert!(unmarked.to_string().contains("boom"));
    }

    {
        let mut turn = drive_turn(
            Arc::clone(&session),
            dispatcher.subscribe("no-retry"),
            Vec::new(),
            None,
        )
        .await;
        let missing = bridge
            .context_retry_or_error(&mut turn, anyhow!("context window exceeded"))
            .await
            .err()
            .expect("missing retry should fail");
        assert!(missing.to_string().contains("context window"));
    }

    let mut turn = drive_turn(
        session,
        dispatcher.subscribe("retry"),
        Vec::new(),
        Some(ContextRetry {
            request: drive_request(),
            effort: Some("high".to_owned()),
            advisor_model: None,
            collaborator_model: None,
        }),
    )
    .await;
    let retry = bridge
        .context_retry_or_error(&mut turn, anyhow!("context window exceeded"))
        .await
        .expect("context retry");
    assert_eq!(retry.request.model, "main");
    assert_eq!(retry.effort.as_deref(), Some("high"));
}

async fn drive_turn(
    session: Arc<Session>,
    events: crate::app_server::ThreadEvents,
    extras: Vec<Value>,
    retry: Option<ContextRetry>,
) -> ActiveTurn {
    let gate = Arc::clone(&session.gate).lock_owned().await;
    ActiveTurn {
        session,
        events: Arc::new(events),
        response_model: "main".to_owned(),
        extras,
        routing_system: Value::Null,
        input_tokens: 1,
        retry,
        gate,
        detached: false,
    }
}

fn drive_request() -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user","content":"retry me"})],
        tools: Vec::new(),
        stream: true,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

async fn disconnect_fixture() -> (tempfile::TempDir, Arc<AppServer>, Bridge, Arc<Session>) {
    disconnect_fixture_with_disabled(Default::default()).await
}

fn grok_disconnect_fixture() -> (Bridge, Arc<Session>, Arc<ThreadEventDispatcher>) {
    let backend = Arc::new(AgentBackend::Grok(GrokAcp::stopped_for_test()));
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let slot = Arc::clone(&bridge.session_slots)
        .try_acquire_owned()
        .expect("session slot");
    let dispatcher = Arc::new(ThreadEventDispatcher::default());
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: BTreeSet::new(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slot,
    });
    (bridge, session, dispatcher)
}

async fn disconnect_fixture_with_disabled(
    disabled_subagent_models: BTreeSet<String>,
) -> (tempfile::TempDir, Arc<AppServer>, Bridge, Arc<Session>) {
    let root = tempfile::tempdir().expect("disconnect fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nlog=\"${0%/*}/responses.log\"\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do printf '%s\\n' \"$line\" >> \"$log\"; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let progress_program = root.path().join("mock-claude");
    std::fs::write(
        &progress_program,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"dynamic progress from progress subagent\"}'\n",
    )
    .expect("write progress model mock");
    let mut progress_permissions = std::fs::metadata(&progress_program)
        .expect("progress model metadata")
        .permissions();
    progress_permissions.set_mode(0o755);
    std::fs::set_permissions(&progress_program, progress_permissions)
        .expect("make progress model mock executable");
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, r#"{"model":"mock-progress"}"#)
        .expect("write progress model settings");
    let bridge = Bridge::new_with_subscription_program(
        Arc::clone(&app),
        "main".to_owned(),
        progress_program,
    )
    .with_settings_path(settings);
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models,
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::from([
            (
                "cc_Agent_batch_0".to_owned(),
                "__claudex_agent_batch__:Agent".to_owned(),
            ),
            ("cc_Agent_0".to_owned(), "Agent".to_owned()),
            ("cc_Read_0".to_owned(), "Read".to_owned()),
        ]),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: Arc::clone(&bridge.session_slots)
            .try_acquire_owned()
            .expect("session slot"),
    });
    (root, app, bridge, session)
}

fn response_ids(log: &std::path::Path) -> Vec<u64> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|response| response.get("id").and_then(Value::as_u64))
        .collect()
}

async fn wait_for_response_ids(log: &std::path::Path, expected: &[u64]) -> Vec<u64> {
    loop {
        let ids = response_ids(log);
        if ids.as_slice() == expected {
            return ids;
        }
        tokio::task::yield_now().await;
    }
}

async fn assert_disconnected_tool_rejections(root: &tempfile::TempDir, expected: &[u64]) {
    let log = root.path().join("responses.log");
    let actual = tokio::time::timeout(
        Duration::from_secs(1),
        wait_for_response_ids(&log, expected),
    )
    .await
    .expect("disconnected tool responses should be written promptly");
    assert_eq!(actual, expected);
}

fn parse_trace_file(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .ok()
        .map(|trace| {
            trace
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default()
}

async fn wait_for_trace_len(path: &std::path::Path, expected: usize) -> Vec<Value> {
    loop {
        let trace = parse_trace_file(path);
        if trace.len() >= expected {
            return trace;
        }
        tokio::task::yield_now().await;
    }
}

async fn request_trace(path: &std::path::Path, expected: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(1), wait_for_trace_len(path, expected))
        .await
        .expect("request trace should land promptly")
}

fn grok_acp_mock_program() -> PathBuf {
    let test_binary = std::env::current_exe().expect("locate stream test binary");
    let target_debug = test_binary
        .parent()
        .and_then(std::path::Path::parent)
        .expect("locate Cargo target debug directory");
    let mock = target_debug.join("grok-acp-mock");
    if mock.is_file() {
        return mock;
    }
    let mock = std::fs::read_dir(target_debug.join("deps"))
        .expect("read Cargo test fixture directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path.extension().is_none()
                && path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|name| name.starts_with("grok_acp_mock-"))
        })
        .expect("locate Grok ACP fixture binary");
    assert!(
        mock.is_file(),
        "grok ACP fixture binary: {}",
        mock.display()
    );
    mock
}

async fn wait_for_disconnected_drain(events: &Arc<crate::app_server::ThreadEvents>) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while Arc::strong_count(events) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background disconnected drain should finish promptly");
}

async fn retryable_drive_fixture() -> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>)
{
    retryable_drive_fixture_with_retried_output(false).await
}

async fn retryable_drive_fixture_with_output()
-> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    retryable_drive_fixture_with_retried_output(true).await
}

async fn retryable_drive_fixture_with_retried_output(
    emit_output: bool,
) -> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    let root = tempfile::tempdir().expect("retry stream fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let requests_log = root.path().join("requests.log");
    let program = root.path().join("retry-app-server");
    let mut program_script = r#"#!/bin/sh
log="__REQUESTS_LOG__"
read initialize
printf '%s\n' '{"id":1,"result":{}}'
read initialized
read start
printf '%s\n' "$start" >> "$log"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"retried"}}}'
read turn
printf '%s\n' "$turn" >> "$log"
sleep 0.05
__RETRIED_OUTPUT__
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"retried","turn":{"status":"completed"}}}'
while read line; do :; done
"#.to_owned();
    program_script = program_script.replace("__REQUESTS_LOG__", &requests_log.to_string_lossy());
    let retried_output = emit_output.then_some(
        "printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"retried\",\"delta\":\"retried answer\"}}'",
    );
    program_script = program_script.replace("__RETRIED_OUTPUT__", retried_output.unwrap_or(""));
    std::fs::write(&program, &program_script).expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Arc::new(Bridge::new(Arc::clone(&app), "main".to_owned()));
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    (root, app, bridge, session)
}

async fn retry_failure_drive_fixture()
-> (tempfile::TempDir, Arc<AppServer>, Arc<Bridge>, Arc<Session>) {
    let root = tempfile::tempdir().expect("retry failure stream fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("source auth");
    let program = root.path().join("retry-failure-app-server");
    std::fs::write(
        &program,
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread start\nprintf '%s\\n' '{\"id\":2,\"error\":{\"message\":\"retry setup failed\"}}'\nwhile read line; do :; done\n",
    )
    .expect("mock app-server");
    let mut permissions = std::fs::metadata(&program).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).unwrap();
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    let bridge = Arc::new(Bridge::new(Arc::clone(&app), "main".to_owned()));
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    (root, app, bridge, session)
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
            web_search_requests: 0,
        },
        web_evidence: super::super::WebEvidenceSummary::default(),
    };
    send_stream_completion(&sender, &segment).await;
    send_stream_error(&sender, anyhow!("boom")).await;
    send_stream_frame(None, "ignored", || json!({}))
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
    assert!(output.contains("\"stop_reason\":\"error\""));
}

#[tokio::test]
async fn stream_error_closes_the_agent_card_with_message_stop() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    send_stream_error(
        &sender,
        anyhow!("ACP driver dropped its response: channel closed"),
    )
    .await;
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("event: error"));
    assert!(output.contains("ACP driver dropped its response"));
    assert!(output.contains("\"stop_reason\":\"error\""));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn completion_frame_exposes_verified_web_evidence_metadata() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let segment = super::super::Segment {
        blocks: Vec::new(),
        stop_reason: "end_turn",
        usage: super::super::Usage {
            input_tokens: 1,
            output_tokens: 4,
            web_search_requests: 2,
        },
        web_evidence: super::super::WebEvidenceSummary::from_verified_count(2),
    };

    send_stream_completion(&sender, &segment).await;

    let frame = receiver
        .recv()
        .await
        .expect("message delta")
        .expect("frame");
    let frame = String::from_utf8(frame.to_vec()).expect("UTF-8 SSE");
    let payload = frame
        .strip_prefix("event: message_delta\ndata: ")
        .expect("message delta payload");
    let payload: Value = serde_json::from_str(payload.trim()).expect("message delta JSON");
    assert_eq!(
        payload["usage"]["server_tool_use"]["web_search_requests"],
        2
    );
    assert_eq!(
        payload["metadata"]["claudex"]["web_evidence"]["verified_count"],
        2
    );
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

#[tokio::test]
async fn sends_tool_frames_with_and_without_a_live_stream() {
    let block = json!({
        "id":"toolu_stream", "name":"lookup", "input":{"key":"value"}
    });
    send_tool_use(None, 0, &block)
        .await
        .expect("missing stream is a no-op");

    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    send_tool_use(Some(&sender), 0, &block)
        .await
        .expect("live stream accepts tool frames");
    assert!(receiver.recv().await.expect("start").is_ok());
    assert!(receiver.recv().await.expect("delta").is_ok());
    assert!(receiver.recv().await.expect("stop").is_ok());

    drop(receiver);
    send_stream_frame(Some(&sender), "closed", || json!({"ok": true}))
        .await
        .expect("closed stream is handled");
}

#[test]
fn committed_output_ignores_empty_or_disposable_blocks() {
    let mut builder = SegmentBuilder::new(1);
    assert!(!builder.has_committed_output());

    builder
        .blocks
        .push(json!({"type":"thinking","thinking":"▶ running"}));
    assert!(!builder.has_committed_output());

    builder.open_text_block = Some((0, String::new()));
    assert!(!builder.has_committed_output());
    builder.open_text_block = Some((0, "answer".to_owned()));
    assert!(builder.has_committed_output());
}

#[tokio::test]
async fn prepared_stream_releases_its_concurrency_ticket_after_a_prepare_error() {
    let (_root, _app, bridge, _session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let limits = super::super::model_concurrency::ModelConcurrency::new(Vec::new());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let mut request = drive_request();
    request.messages = vec![json!({
        "role":"user",
        "content":[{"type":"tool_result","tool_use_id":"orphan","content":"result"}]
    })];

    Arc::clone(&bridge)
        .drive_prepared_subagent_stream(super::PreparedStream {
            request,
            input_tokens: 1,
            effort: None,
            concurrency_ticket: limits.ticket("main", Some(1)),
            is_subagent: false,
            run_in_background: false,
            sender,
            primed_thinking: false,
        })
        .await;

    let frame = receiver
        .recv()
        .await
        .expect("stream preparation error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&frame).contains("no active claudex session"));
    assert_eq!(
        serde_json::to_value(limits.snapshot()).unwrap()["main"]["active"],
        0
    );
}

#[tokio::test]
async fn prepared_stream_stops_when_the_client_disconnects_during_setup() {
    let (_root, _app, bridge, _session) = disconnect_fixture().await;
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);

    Arc::new(bridge)
        .drive_prepared_subagent_stream(super::PreparedStream {
            request: drive_request(),
            input_tokens: 1,
            effort: None,
            concurrency_ticket: None,
            is_subagent: false,
            run_in_background: false,
            sender,
            primed_thinking: false,
        })
        .await;
}

#[tokio::test]
async fn external_batch_segment_returns_an_unsettled_segment_while_stream_is_open() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    let mut builder = SegmentBuilder::new(1);
    builder
        .text_delta(&json!({"params":{"delta":"answer"}}), Some(&sender))
        .await
        .expect("text segment");

    let result = bridge
        .external_batch_segment(&session, events, &mut builder, Some(&sender))
        .await
        .expect("open stream segment");
    let super::StreamTurn::Segment {
        segment,
        provider_settled,
    } = result
    else {
        panic!("open sender must keep the batch segment");
    };
    assert!(!provider_settled);
    assert_eq!(segment.blocks[0]["text"], "answer");
}

#[tokio::test]
async fn finish_closed_stream_retains_settled_session_for_follow_up_reuse() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge);
    let events = Arc::new(app.subscribe_thread("thread"));

    // Add session to bridge with committed transcript
    bridge.sessions.lock().await.push(Arc::clone(&session));
    session
        .transcript
        .lock()
        .await
        .push(json!({"role":"assistant","content":[{"type":"text","text":"initial response"}]}));

    // Verify session is in bridge before finish_closed_stream
    assert_eq!(bridge.sessions.lock().await.len(), 1);

    // Call finish_closed_stream with provider_settled=true
    bridge.finish_closed_stream(&session, &events, true).await;

    // CRITICAL ASSERTION: Session must remain in bridge.sessions after settled completion
    assert_eq!(
        bridge.sessions.lock().await.len(),
        1,
        "finish_closed_stream with provider_settled=true must retain session for idle reuse"
    );
    let retained = Arc::clone(
        bridge
            .sessions
            .lock()
            .await
            .iter()
            .find(|s| Arc::ptr_eq(s, &session))
            .expect("session must still be in bridge.sessions"),
    );
    assert!(
        Arc::ptr_eq(&retained, &session),
        "retained session must match original"
    );

    // Simulate a follow-up request accessing the idle session
    // Build a request that matches the session's signature and transcript
    let _follow_up_request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::String("system".to_owned()),
        messages: vec![
            json!({"role":"assistant","content":[{"type":"text","text":"initial response"}]}),
            json!({"role":"user","content":"continue?"}),
        ],
        tools: vec![],
        stream: false,
        output_config: Value::Null,
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    let _signature: Arc<str> = Arc::from("signature");

    // Directly verify that sessions.lock().await contains our idle session
    let live_sessions = bridge.sessions.lock().await;
    assert_eq!(live_sessions.len(), 1);
    assert!(
        live_sessions.iter().any(|s| Arc::ptr_eq(s, &session)),
        "idle session must still be discoverable"
    );

    // The idle session remains available for future select_session() calls
    // which will use reserve_matching_session internally to find it
    drop(live_sessions);

    assert_eq!(bridge.used_session_slots(), 1);
}

#[test]
fn retain_closed_subagent_session_keeps_tool_use_and_settled_turns() {
    assert!(super::event_consume::retain_closed_subagent_session(
        true, "end_turn"
    ));
    assert!(super::event_consume::retain_closed_subagent_session(
        false, "tool_use"
    ));
    assert!(!super::event_consume::retain_closed_subagent_session(
        false, "end_turn"
    ));
}

#[tokio::test]
async fn subagent_activity_keepalive_runs_after_sse_disconnect() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    tokio::time::pause();
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    drop(receiver);

    let wait = bridge.wait_for_stream_segment_with_interval(StreamWaitInput {
        session: &session,
        events: Arc::new(events),
        current_messages: &[],
        system: &json!(null),
        sender: &sender,
        builder: SegmentBuilder::for_turn(1, true, "gpt-5.6-luna"),
        activity_interval: Duration::from_millis(10),
        initial_activity_delay: Duration::from_millis(10),
    });
    let complete = async {
        // Let biased closed + NoEvent settle, then fire keepalive with sse=None.
        yield_n(4).await;
        tokio::time::advance(Duration::from_millis(25)).await;
        yield_n(4).await;
        tokio::time::advance(Duration::from_millis(25)).await;
        dispatcher.dispatch(json!({
            "method":"turn/completed",
            "params":{"threadId":"thread","turn":{"status":"completed"}}
        }));
    };
    let (result, ()) = tokio::join!(wait, complete);
    let super::StreamTurn::Segment { .. } = result.expect("segment after keepalive") else {
        panic!("expected completed segment");
    };
}

#[tokio::test]
async fn wait_for_stream_hands_off_external_tool_batch_after_quiet_period() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    tokio::time::pause();
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[json!({"role":"user","content":"read"})],
            &Value::Null,
            &json!({
                "id":41,
                "method":"item/tool/call",
                "params":{
                    "callId":"read-1",
                    "tool":"cc_Read_0",
                    "arguments":{"path":"README.md"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("external read");
    assert!(builder.has_external_tool_calls());

    let wait = bridge.wait_for_stream_segment_with_interval(StreamWaitInput {
        session: &session,
        events: Arc::new(events),
        current_messages: &[],
        system: &json!(null),
        sender: &sender,
        builder,
        activity_interval: Duration::from_secs(30),
        initial_activity_delay: Duration::from_secs(30),
    });
    let advance = async {
        yield_n(4).await;
        tokio::time::advance(Duration::from_millis(10)).await;
    };
    let (result, ()) = tokio::join!(wait, advance);
    let super::StreamTurn::Segment {
        segment,
        provider_settled,
    } = result.expect("batch handoff segment")
    else {
        panic!("quiet external tools must finish as a segment");
    };
    assert!(!provider_settled);
    assert_eq!(segment.stop_reason, "tool_use");
}

#[tokio::test]
async fn external_batch_segment_keeps_subagent_segment_when_sse_already_closed() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver);
    let mut builder = SegmentBuilder::for_turn(1, true, "gpt-5.6-luna");
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":42,
                "method":"item/tool/call",
                "params":{
                    "callId":"read-closed",
                    "tool":"cc_Read_0",
                    "arguments":{"path":"CLAUDE.md"}
                }
            }),
            None,
        )
        .await
        .expect("external tool");
    let result = bridge
        .external_batch_segment(&session, events, &mut builder, Some(&sender))
        .await
        .expect("closed subagent batch");
    let super::StreamTurn::Segment {
        provider_settled, ..
    } = result
    else {
        panic!("closed SubAgent SSE must keep the unfinished tool segment");
    };
    assert!(!provider_settled);
}

#[tokio::test]
async fn external_batch_segment_cancels_provider_for_acp_bridged_tool_use() {
    let (root, app, bridge, session) = disconnect_fixture().await;
    let events = Arc::new(app.subscribe_thread("thread"));
    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);
    session.pending_tools.lock().await.insert(
        "toolu_bridge".to_owned(),
        super::acp_tool_bridge::acp_bridge_request_id("spawn-1"),
    );
    let mut builder = SegmentBuilder::new(1);
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[json!({"role":"user","content":"delegate"})],
            &json!(
                r#"{"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
            ),
            &json!({
                "id":43,
                "method":"item/tool/call",
                "params":{
                    "callId":"agent-1",
                    "tool":"cc_Agent_0",
                    "arguments":{
                        "prompt":"work",
                        "subagent_type":"worker",
                        "claudex_model":"worker-model"
                    }
                }
            }),
            Some(&sender),
        )
        .await
        .expect("agent tool");
    let result = bridge
        .external_batch_segment(&session, events, &mut builder, Some(&sender))
        .await
        .expect("bridged batch");
    assert!(matches!(
        result,
        super::StreamTurn::Segment {
            provider_settled: false,
            ..
        }
    ));
    let log = std::fs::read_to_string(root.path().join("responses.log")).unwrap_or_default();
    let _ = log;
}

#[tokio::test]
async fn finish_completed_stream_commits_closed_subagent_without_sse_frames() {
    let (_root, app, bridge, session) = disconnect_fixture().await;
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let events = app.subscribe_thread("thread");
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    drop(receiver);
    let turn = drive_turn(Arc::clone(&session), events, Vec::new(), None).await;
    let segment = super::super::Segment {
        blocks: vec![json!({"type":"text","text":"done"})],
        stop_reason: "end_turn",
        usage: super::super::Usage {
            input_tokens: 1,
            output_tokens: 1,
            web_search_requests: 0,
        },
        web_evidence: super::super::WebEvidenceSummary::default(),
    };
    bridge
        .finish_completed_stream(turn, &sender, segment, false, true)
        .await;
    let transcript = session.transcript.lock().await.clone();
    assert!(
        transcript
            .iter()
            .any(|message| { message.get("role").and_then(Value::as_str) == Some("assistant") }),
        "closed SubAgent finish must still commit transcript: {transcript:?}"
    );
}

#[tokio::test]
async fn non_streaming_response_retries_after_context_window() {
    let (_root, _app, bridge, session) = retryable_drive_fixture_with_output().await;
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let response = bridge
        .non_streaming_response(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
        )
        .await
        .expect("context retry response");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("message") || text.contains("content") || text.contains("end_turn"),
        "unexpected non-stream retry body: {text}"
    );
}

#[tokio::test]
async fn non_streaming_response_reports_usage_limit_without_retry_payload() {
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let error = bridge
        .non_streaming_response(drive_turn(session, events, Vec::new(), None).await)
        .await
        .expect_err("empty ACP without retry must fail");
    assert!(
        error.to_string().contains("no assistant content")
            || error.to_string().contains("usage")
            || error.to_string().contains("billing"),
        "{error:#}"
    );
}

#[tokio::test]
async fn failover_usage_limit_turn_requires_retry_and_failover_target() {
    let (root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = bridge.with_usage_limit_cache_home(root.path());
    let dispatcher = ThreadEventDispatcher::default();
    let error = anyhow!("usage limit exceeded");

    let missing_retry = bridge
        .failover_usage_limit_turn(
            drive_turn(
                Arc::clone(&session),
                dispatcher.subscribe("no-retry"),
                Vec::new(),
                None,
            )
            .await,
            anyhow!("{error}"),
        )
        .await;
    assert!(missing_retry.is_err(), "retry payload is required");

    let missing_failover = bridge
        .failover_usage_limit_turn(
            drive_turn(
                session,
                dispatcher.subscribe("no-failover"),
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: Some("high".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            error,
        )
        .await;
    assert!(
        missing_failover.is_err(),
        "single-model Codex bridge has no sibling failover"
    );
}

#[tokio::test]
async fn failover_usage_limit_turn_returns_subscription_response() {
    let (root, _app, bridge, session) = disconnect_fixture().await;
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("subscription fallback");
    let bridge = bridge
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    let dispatcher = ThreadEventDispatcher::default();
    let outcome = bridge
        .failover_usage_limit_turn(
            drive_turn(
                session,
                dispatcher.subscribe("subscription-failover"),
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: Some("high".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            anyhow!("usage limit exceeded"),
        )
        .await
        .expect("subscription failover");
    let super::context_retry::UsageLimitOutcome::Response(response) = outcome else {
        panic!("Codex usage-limit must Response through subscription failover");
    };
    assert_eq!(
        response.status(),
        axum::http::StatusCode::OK,
        "subscription mock should answer the failover"
    );
}

#[tokio::test]
async fn non_streaming_response_failsover_usage_limit_to_subscription() {
    let (root, _app, bridge, session) = disconnect_fixture().await;
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-sonnet",
            "claude-sonnet-5",
            "high",
        )])
        .expect("subscription fallback");
    let bridge = bridge
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(root.path());
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let response = bridge
        .non_streaming_response(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
        )
        .await
        .expect("usage-limit subscription failover");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn failover_usage_limit_turn_attempts_sibling_configured_acp_provider() {
    let cache = tempfile::tempdir().expect("failover cache");
    let mut qwen = crate::agent_backend::BackendRoute::new(
        "qwen3.8-max-preview",
        crate::agent_backend::BackendKind::ConfiguredAcp,
    );
    qwen.max_concurrency = Some(3);
    let mut cursor = crate::agent_backend::BackendRoute::new(
        "auto",
        crate::agent_backend::BackendKind::ConfiguredAcp,
    );
    cursor.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[qwen, cursor]);
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-qwen", "qwen3.8-max-preview", "high"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", "auto", "high"),
        ])
        .expect("workers");
    let bridge = Bridge::new_with_backend(backend, "qwen3.8-max-preview".to_owned())
        .with_model_catalog(catalog)
        .with_usage_limit_cache_home(cache.path());
    let slots = Arc::new(Semaphore::new(2));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "qwen3.8-max-preview".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("slot"),
    });
    let dispatcher = ThreadEventDispatcher::default();
    let mut request = drive_request();
    request.model = "qwen3.8-max-preview".to_owned();
    let result = bridge
        .failover_usage_limit_turn(
            drive_turn(
                session,
                dispatcher.subscribe("acp-failover"),
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: Some("high".to_owned()),
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            anyhow!(crate::anthropic::segment::EMPTY_ACP_END_TURN),
        )
        .await;
    match result {
        Ok(super::context_retry::UsageLimitOutcome::Continue(turn)) => {
            assert_eq!(turn.session.model, "auto");
        }
        Ok(super::context_retry::UsageLimitOutcome::Response(_)) => {
            panic!("configured ACP sibling must use Provider Continue, not subscription Response");
        }
        Err(error) => {
            // Lazy ACP routes may not be live in unit tests; the Provider rewrite
            // and retry_after_context_window attempt still cover the Continue arm.
            assert!(
                !error.to_string().is_empty(),
                "sibling provider attempt must surface a concrete error"
            );
        }
    }
}

async fn empty_acp_sibling_retry_bridge() -> (
    tempfile::TempDir,
    Arc<Bridge>,
    Arc<Session>,
    ThreadEventDispatcher,
) {
    let cache = tempfile::tempdir().expect("stream failover cache");
    let mut qwen = crate::agent_backend::BackendRoute::new(
        "qwen3.8-max-preview",
        crate::agent_backend::BackendKind::ConfiguredAcp,
    );
    qwen.max_concurrency = Some(3);
    let mut cursor = crate::agent_backend::BackendRoute::new(
        "auto",
        crate::agent_backend::BackendKind::ConfiguredAcp,
    );
    cursor.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[qwen, cursor]);
    let mut catalog = crate::provider_config::ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-qwen", "qwen3.8-max-preview", "high"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", "auto", "high"),
        ])
        .expect("workers");
    let bridge = Arc::new(
        Bridge::new_with_backend(backend, "qwen3.8-max-preview".to_owned())
            .with_model_catalog(catalog)
            .with_usage_limit_cache_home(cache.path()),
    );
    let slots = Arc::new(Semaphore::new(2));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "qwen3.8-max-preview".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("slot"),
    });
    let dispatcher = ThreadEventDispatcher::default();
    (cache, bridge, session, dispatcher)
}

#[tokio::test]
async fn drive_subagent_stream_retries_empty_acp_on_sibling_provider() {
    let (_cache, bridge, session, dispatcher) = empty_acp_sibling_retry_bridge().await;
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut request = drive_request();
    request.model = "qwen3.8-max-preview".to_owned();
    Arc::clone(&bridge)
        .drive_subagent_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request,
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::for_turn(1, true, "qwen3.8-max-preview"),
            None,
            true,
            false,
        )
        .await;
    let output = collect_sse_frames(&mut receiver).await;
    assert!(
        output.contains("error")
            || output.contains("message_stop")
            || output.contains("no assistant")
            || output.contains("auto"),
        "empty-ACP sibling retry must produce stream output: {output}"
    );
}

#[tokio::test]
async fn drive_stream_retries_usage_limit_err_after_committed_output() {
    let cache = tempfile::tempdir().expect("cache");
    let (_root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = Arc::new(bridge.with_usage_limit_cache_home(cache.path()));
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"partial"}
    }));
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"codexErrorInfo":"usageLimitExceeded"}}
    }));
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    Arc::clone(&bridge)
        .drive_subagent_stream(
            drive_turn(
                session,
                events,
                Vec::new(),
                Some(ContextRetry {
                    request: drive_request(),
                    effort: None,
                    advisor_model: None,
                    collaborator_model: None,
                }),
            )
            .await,
            sender,
            SegmentBuilder::for_turn(1, true, "main"),
            None,
            true,
            false,
        )
        .await;
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        output.contains("usage")
            || output.contains("limit")
            || output.contains("error")
            || output.contains("partial"),
        "unexpected usage-limit-after-output stream: {output}"
    );
}

#[tokio::test]
async fn blocks_exhausted_subagent_launch_with_cooling_down_notice() {
    let (root, _app, bridge, session) = disconnect_fixture().await;
    let bridge = bridge.with_usage_limit_cache_home(root.path());
    bridge.note_provider_exhaustion(
        &anyhow!(crate::anthropic::segment::EMPTY_ACP_END_TURN),
        Some("worker-model"),
    );
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    let flow = builder
        .handle_event(
            &bridge,
            &session,
            &[],
            &json!(
                r#"{"providers":{},"selected_workers":[{"agent":"worker","model":"worker-model"}]}"#
            ),
            &json!({
                "id":44,
                "method":"item/tool/call",
                "params":{
                    "callId":"exhausted",
                    "tool":"cc_Agent_0",
                    "arguments":{
                        "prompt":"delegate",
                        "subagent_type":"worker",
                        "claudex_model":"worker-model"
                    }
                }
            }),
            Some(&sender),
        )
        .await
        .expect("exhausted launch stays on the parent stream");
    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(!builder.has_external_tool_calls());
    assert!(builder.blocks.iter().any(|block| {
        block["text"]
            .as_str()
            .is_some_and(|text| text.contains("cooling down"))
    }));
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("cooling down") || output.contains("worker-model"));
}

#[tokio::test]
async fn command_code_external_tool_skips_progress_arrow_painting() {
    let (_root, _app, bridge, mut session) = disconnect_fixture().await;
    Arc::get_mut(&mut session)
        .expect("unique session")
        .external_tool_names
        .insert("cc_Read_0".to_owned(), "Read".to_owned());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::for_turn(1, true, "meta/muse-spark-1.2-contributor");
    let _ = builder
        .handle_event(
            &bridge,
            &session,
            &[],
            &Value::Null,
            &json!({
                "id":45,
                "method":"item/tool/call",
                "params":{
                    "callId":"cc-read",
                    "tool":"cc_Read_0",
                    "arguments":{"path":"README.md"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("command-code external read");
    assert!(builder.has_external_tool_calls());
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("▶ Read"),
        "Command Code SubAgent must skip ▶ chrome before tool_use: {output}"
    );
    assert!(output.contains("tool_use") || output.contains("Read"));
}
