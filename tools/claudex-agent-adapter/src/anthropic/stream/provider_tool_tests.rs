// Coverage off is on the parent `#[path]` mod in provider_tool.rs.

use axum::body::Bytes;
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use tokio::sync::mpsc;

use super::*;

#[tokio::test]
async fn builds_provider_progress_without_executable_tool_use_blocks() {
    const SAMPLE_INPUT_TOKEN_COUNT: u64 = 1;
    const EXPECTED_PROVIDER_CALLS: usize = 2;
    const EXPECTED_PROGRESS_BLOCKS: usize = 1;
    let mut builder = SegmentBuilder::new(SAMPLE_INPUT_TOKEN_COUNT);
    assert!(builder.provider_tool_call(&json!({}), None).await.is_err());
    assert!(
        builder
            .provider_tool_call(&json!({"params":{}}), None)
            .await
            .is_err()
    );
    builder
        .provider_tool_call(
            &json!({"params":{"callId":"provider-read","tool":"Read","arguments":{"path":"a"}}}),
            None,
        )
        .await
        .expect("provider progress");
    // ACP may describe the same call again in an incremental update.
    builder
        .provider_tool_call(
            &json!({"params":{"callId":"provider-read","tool":"Read","title":"Read a"}}),
            None,
        )
        .await
        .expect("duplicate provider progress");
    builder
        .provider_tool_call(
            &json!({"params":{"callId":"provider-search","title":"Search docs"}}),
            None,
        )
        .await
        .expect("default provider progress");

    // Progress is visible in thinking chrome before commit (never tool_use).
    assert_eq!(builder.blocks.len(), EXPECTED_PROGRESS_BLOCKS);
    assert!(builder.open_text_block.is_none());
    let thinking = thinking_text(&builder);
    assert!(thinking.contains("▶ Read"));
    assert!(thinking.contains("a"));
    assert!(thinking.contains("▶ Search docs"));
    assert!(!thinking.contains("tool_use"));
    assert_eq!(builder.provider_tool_calls.len(), EXPECTED_PROVIDER_CALLS);
    let segment = builder.finish(None).await.expect("segment");
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| !committed_progress_text(block).contains('▶'))
    );
}

#[tokio::test]
async fn dedupes_replayed_provider_completion_marker_by_call_id() {
    let mut builder = SegmentBuilder::new(1);
    for _ in 0..2 {
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":"replayed-completion",
                    "status":"completed",
                    "title":"Read"
                }}),
                None,
            )
            .await
            .expect("provider completion");
    }
    assert_eq!(thinking_text(&builder).matches("✓ Read").count(), 1);
}

#[tokio::test]
async fn dedupes_replayed_provider_failure_marker_by_call_id() {
    let mut builder = SegmentBuilder::new(1);
    for _ in 0..2 {
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":"replayed-failure",
                    "status":"failed",
                    "title":"Bash",
                    "output":"boom"
                }}),
                None,
            )
            .await
            .expect("provider failure");
    }
    assert_eq!(thinking_text(&builder).matches("✗ Bash").count(), 1);
}

#[tokio::test]
async fn provider_tool_start_update_complete_paints_one_start_and_terminal_marker() {
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"one-lifecycle",
                "tool":"Read",
                "title":"Read"
            }}),
            None,
        )
        .await
        .expect("provider tool call");
    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"one-lifecycle",
                "status":"in_progress",
                "title":"Read"
            }}),
            None,
        )
        .await
        .expect("provider tool update");
    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"one-lifecycle",
                "status":"completed",
                "title":"Read"
            }}),
            None,
        )
        .await
        .expect("provider completion");
    let thinking = thinking_text(&builder);
    assert_eq!(thinking.matches("▶ Read").count(), 1);
    assert_eq!(thinking.matches("✓ Read").count(), 1);
}

#[tokio::test]
async fn streams_provider_progress_and_all_status_variants() {
    const SAMPLE_INPUT_TOKEN_COUNT: u64 = 1;
    const STREAM_CHANNEL_CAPACITY: usize = 32;
    const EXPECTED_PROGRESS_FRAMES: usize = 7;
    let (sender, mut receiver) =
        mpsc::channel::<Result<Bytes, Infallible>>(STREAM_CHANNEL_CAPACITY);
    let mut builder = SegmentBuilder::new(SAMPLE_INPUT_TOKEN_COUNT);
    builder
        .provider_tool_call(
            &json!({"params":{"callId":"1","tool":"Bash","arguments":{}}}),
            Some(&sender),
        )
        .await
        .expect("stream progress");
    builder
        .provider_tool_update(
            &json!({"params":{"status":"failed","title":"Build","output":{"code":1}}}),
            Some(&sender),
        )
        .await
        .expect("failed status");
    builder
        .provider_tool_update(
            &json!({"params":{"status":"completed","title":"Read","output":" done "}}),
            Some(&sender),
        )
        .await
        .expect("completed status");
    builder
        .provider_tool_update(&json!({"params":{"status":"completed"}}), Some(&sender))
        .await
        .expect("empty completed status");
    builder
        .provider_tool_update(&json!({"params":{"status":"pending"}}), Some(&sender))
        .await
        .expect("ignored status");
    assert!(
        builder
            .provider_tool_update(&json!({}), None)
            .await
            .is_err()
    );
    let segment = builder.finish(Some(&sender)).await.expect("segment");
    drop(sender);

    assert!(
        segment
            .blocks
            .iter()
            .all(|block| block["type"] != "tool_use")
    );
    // Live thinking_delta carries progress; committed output is transcript-clean.
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| committed_progress_text(block).trim().is_empty())
    );
    let (frame_count, output) = collect_frames(&mut receiver).await;
    assert!(output.contains("thinking_delta"));
    assert!(output.contains("▶ Bash"));
    assert!(output.contains("✗ Build"));
    assert!(output.contains("✓ Read"));
    assert!(!output.contains(" done "));
    assert_eq!(frame_count, EXPECTED_PROGRESS_FRAMES);
}

#[tokio::test]
async fn completed_progress_omits_large_tool_bodies() {
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"shell-1",
                "tool":"run_terminal_command",
                "title":"run_terminal_command",
                "arguments":{"command":"pwd && git status && git branch --show-current && ls -la"}
            }}),
            None,
        )
        .await
        .expect("start");
    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"shell-1",
                "status":"completed",
                "title":"run_terminal_command",
                "output":{"command":"pwd && git status","stdout":"huge\n".repeat(200),"exitCode":0}
            }}),
            None,
        )
        .await
        .expect("complete");
    let thinking = thinking_text(&builder);
    assert!(thinking.contains("▶ run_terminal_command"));
    assert!(thinking.contains("✓ run_terminal_command"));
    assert!(!thinking.contains("exitCode"));
    assert!(!thinking.contains("huge"));
    assert!(!thinking.contains("stdout"));
    // Success line is marker-only (no tool body JSON).
    assert!(!thinking.contains("✓ run_terminal_command:"));
}

#[tokio::test]
async fn renders_update_only_tools_once_and_reuses_their_titles() {
    let mut builder = SegmentBuilder::new(1);
    send_update_statuses(&mut builder).await;
    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"update-only",
                "status":"completed",
                "output":"done"
            }}),
            None,
        )
        .await
        .expect("provider completion");
    // Progress is accumulated once in thinking, then removed from commit.
    assert_eq!(thinking_text(&builder).matches("▶ WebFetch").count(), 1);
    let segment = builder.finish(None).await.expect("segment");
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| committed_progress_text(block).trim().is_empty())
    );
}

#[tokio::test]
async fn qwen_mid_turn_status_then_read_then_complete_is_visible_before_finish() {
    // Live Qwen SubAgent (d2945e45 / a233508547ce0d5c6): AgentMessage status,
    // then ReadFile, then ✓, then more status — all before end_turn.
    // Old failure: ▶ arrived only in committed text after finish, so the
    // panel stayed on blank "Thought for Xs" + spinner during the turn.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = LiveView::default();
    run_qwen_mid_turn_events(&mut builder, &sender, &mut receiver, &mut live).await;
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.stop_reason, "end_turn");
    assert!(
        segment.blocks.iter().any(text_has_starting_inspection),
        "answer prose must flush at end_turn: {:?}",
        segment.blocks
    );
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| !committed_progress_text(block).contains('▶')
                && !committed_progress_text(block).contains('✓')),
        "committed transcript stays clean: {:?}",
        segment.blocks
    );
    live.ingest_available(&mut receiver);
    assert!(
        live.turn_still_open(),
        "SegmentBuilder::finish does not emit message_stop; drive_stream does"
    );
    assert!(
        (live.visible_thinking.contains('▶') && live.visible_thinking.contains('✓'))
            || live
                .visible_server_tools
                .iter()
                .any(|name| name.contains("text_editor") || name.contains("bash")),
        "mid-turn chrome must remain after finish sanitize: thinking={:?} server_tools={:?}",
        live.visible_thinking,
        live.visible_server_tools
    );
}

#[tokio::test]
async fn cline_mid_turn_read_progress_is_visible_before_finish() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"cline:reasoning","summaryIndex":0,"delta":"\n\n\n"}
            }),
            Some(&sender),
        )
        .await
        .expect("ignore blank thought");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.trim().is_empty(),
        "blank Cline thought must not occupy chrome: {:?}",
        live.visible_thinking
    );

    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"read-queue",
                "tool":"Read",
                "title":"Read queue-consumer.ts",
                "arguments":{"path":"/Users/kkk4oru/ghq/github.com/kkkaoru/horse-racing-data/apps/finish-position-cron/src/queue-consumer.ts"}
            }}),
            Some(&sender),
        )
        .await
        .expect("read progress");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains("▶ Read"),
        "{:?}",
        live.visible_thinking
    );
    assert!(
        live.visible_thinking.contains("queue-consumer.ts"),
        "{:?}",
        live.visible_thinking
    );
    assert!(!live.hidden_text.contains('▶'));
}

#[tokio::test]
async fn qwen_agent_message_then_tool_progress_stays_in_thinking_chrome() {
    // Live Qwen SubAgent: AgentMessageChunk opens text, then ReadFile/Grep.
    // Old stream_progress_text appended ▶ to hidden text_delta.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .text_delta(
            &json!({"params":{
                "delta":"Phase 1: reading CLAUDE.md.\n",
                "itemId":"qwen:message"
            }}),
            Some(&sender),
        )
        .await
        .expect("qwen message");
    assert!(builder.open_text_block.is_none());
    assert!(
        builder
            .pending_answer
            .contains("Phase 1: reading CLAUDE.md")
    );
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"qwen-read",
                "tool":"Read",
                "title":"ReadFile: /Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/CLAUDE.md",
                "arguments":{"file_path":"/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/CLAUDE.md"}
            }}),
            Some(&sender),
        )
        .await
        .expect("qwen read progress");
    assert!(
        builder.open_text_block.is_none(),
        "SubAgent answer stays pending until end_turn"
    );
    let thinking = thinking_text(&builder);
    assert!(
        thinking.contains("Phase 1: reading CLAUDE.md"),
        "{thinking}"
    );
    drop(sender);
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(
        output.contains("\"type\":\"server_tool_use\"")
            || output.contains('▶'),
        "Read progress must be server card or ▶: {output}"
    );
    assert!(
        !output.contains("\"type\":\"text_delta\",\"text\":\"\\n▶"),
        "▶ must not ride hidden text_delta: {output}"
    );
}

#[tokio::test]
async fn qwen_prose_progress_in_agent_message_becomes_thinking_delta() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .text_delta(
            &json!({"params":{
                "delta":"\n▶ ReadFile\n",
                "itemId":"qwen:message"
            }}),
            Some(&sender),
        )
        .await
        .expect("prose progress");
    assert!(builder.open_text_block.is_none());
    let thinking = thinking_text(&builder);
    assert!(thinking.contains("▶ ReadFile"), "{thinking}");
    drop(sender);
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(output.contains("thinking_delta"));
    assert!(output.contains("▶ ReadFile"));
    assert!(!output.contains("\"type\":\"text_delta\""));
}

#[tokio::test]
async fn cline_whitespace_thought_then_read_shows_progress_not_blank_thought() {
    // Live TUI (fa522331 / cline-pass/deepseek-v4-flash):
    // `Thought for 8s` + blank lines + `Spinning…` with no ▶ Read.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1);
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"cline:reasoning","summaryIndex":0,"delta":"\n\n\n"}
            }),
            Some(&sender),
        )
        .await
        .expect("ignore blank thought");
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"read-queue",
                "tool":"Read",
                "title":"Read queue-consumer.ts",
                "arguments":{"path":"/Users/kkk4oru/ghq/github.com/kkkaoru/horse-racing-data/apps/finish-position-cron/src/queue-consumer.ts"}
            }}),
            Some(&sender),
        )
        .await
        .expect("read progress");
    drop(sender);
    let thinking = thinking_text(&builder);
    assert!(thinking.contains("▶ Read"), "{thinking}");
    assert!(thinking.contains("queue-consumer.ts"), "{thinking}");
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(output.contains("thinking_delta"));
    assert!(output.contains("▶ Read"));
    assert!(
        !output.contains("\"thinking\":\"\\n\\n\\n\""),
        "blank Cline thought must not occupy the SubAgent thinking chrome"
    );
}

#[tokio::test]
async fn provider_tool_progress_closes_model_thought_so_status_is_visible() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1);
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{"itemId":"cline:reasoning","summaryIndex":0,"delta":"Trace target_race next."}
            }),
            Some(&sender),
        )
        .await
        .expect("model thought");
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"grep-1",
                "tool":"Grep",
                "title":"Grep target_race",
                "arguments":{"pattern":"target_race"}
            }}),
            Some(&sender),
        )
        .await
        .expect("grep progress");
    drop(sender);
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(output.contains("Trace target_race next."));
    assert!(
        output.contains("signature_delta"),
        "CoT unit must close before ▶ progress"
    );
    assert!(output.contains("▶ Grep"));
    assert!(output.contains("target_race"));
}

#[tokio::test]
async fn bash_title_containing_task_still_streams_thinking_progress() {
    // Old failure: title `schtasks` / "agent history" looked launch-shaped,
    // WIP was suppressed or parked in assistant text that SubAgent TUI hides.
    let mut builder = SegmentBuilder::new(1);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"bash-schtasks",
                "tool":"Bash",
                "title":"`cd /repo && prlctl exec Windows cmd.exe /c schtasks /Query /TN \"PC-KEIBA Auto Update\"`",
                "arguments":{"command":"prlctl exec Windows schtasks /Query"}
            }}),
            Some(&sender),
        )
        .await
        .expect("schtasks bash progress");
    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"bash-schtasks",
                "status":"completed",
                "title":"`cd /repo && prlctl exec Windows cmd.exe /c schtasks /Query`"
            }}),
            Some(&sender),
        )
        .await
        .expect("schtasks bash complete");
    drop(sender);
    assert!(builder.open_text_block.is_none());
    let thinking = thinking_text(&builder);
    assert!(thinking.contains("▶ Bash") || thinking.contains("▶ `cd"));
    assert!(thinking.contains('✓'));
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(output.contains("thinking_delta"));
    assert!(output.contains('▶'));
    let segment = builder.finish(None).await.expect("segment");
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| !committed_progress_text(block).contains('▶'))
    );
}

#[tokio::test]
async fn nucleating_cursor_subagent_is_replaced_by_bash_thinking_progress() {
    // Old failure: Cursor SubAgent panel stayed on "Nucleating…" while Bash
    // ran, because canned chrome was kept and ▶ was not painted as thinking.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{
                    "itemId":"cursor:reasoning",
                    "summaryIndex":0,
                    "delta":"Nucleating…"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("nucleating chrome");
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"bash-ls",
                "tool":"Bash",
                "title":"`ls /tmp/claude-5a7a0dcd/tasks`",
                "arguments":{"command":"ls /tmp/claude-5a7a0dcd/tasks"}
            }}),
            Some(&sender),
        )
        .await
        .expect("bash progress");
    drop(sender);
    let (_, output) = collect_frames(&mut receiver).await;
    assert!(
        output.contains("bash_code_execution") || output.contains('▶'),
        "Bash must paint server card or ▶: {output}"
    );
    assert!(!output.to_ascii_lowercase().contains("nucleating"));
}

#[test]
fn previews_and_truncates_status_output() {
    const UNREACHED_PREVIEW_CHAR_LIMIT: usize = 20;
    const TRUNCATED_PREVIEW_CHAR_LIMIT: usize = 3;
    assert_eq!(failure_preview(Some(&json!("text"))), "text");
    assert_eq!(
        failure_preview(Some(&json!({"error":"boom\nmore"}))),
        "boom"
    );
    assert_eq!(failure_preview(None), "failed");
    assert_eq!(
        compact_title("run_terminal_command: pwd && git status"),
        "run_terminal_command"
    );
    assert_eq!(
        compact_title(": only-detail"),
        ": only-detail",
        "empty names after ':' must keep the full title"
    );
    assert_eq!(
        progress_start_line("Shell", Some(&json!(["not-an-object"]))),
        "\n▶ Shell\n"
    );
    assert_eq!(
        progress_start_line("Shell", Some(&json!({"command":"  "}))),
        "\n▶ Shell\n"
    );
    assert_eq!(failure_preview(Some(&json!(["x"]))), "failed");
    assert_eq!(failure_preview(Some(&json!(42))), "failed");
    assert_eq!(
        failure_preview(Some(&json!({"error":"","message":" \n","stderr":"\t"}))),
        "failed",
        "blank object fields must fall through to the generic failure label"
    );
    assert!(scalar_preview(&json!(null)).is_none());
    assert!(scalar_preview(&json!([])).is_none());
    assert!(scalar_preview(&json!("   ")).is_none());
    assert_eq!(
        truncate_for_status("  short  ", UNREACHED_PREVIEW_CHAR_LIMIT),
        "short"
    );
    assert_eq!(
        truncate_for_status("abcdef", TRUNCATED_PREVIEW_CHAR_LIMIT),
        "abc…"
    );
}

#[tokio::test]
async fn cline_agent_message_prose_is_visible_before_end_turn() {
    // Live TUI (fa522331 / cline-pass/deepseek-v4-flash Viewer KV/Cache):
    // AgentMessage prose stayed in hidden text_delta, so the SubAgent viewer
    // was blank for 10+ minutes while tokens were still flowing.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{
                    "itemId":"cline:message",
                    "delta":"型と配信パスを把握しました。既存 finish-prediction-inputs-cache の接続状況を確認します。\n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("cline prose");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains("型と配信パスを把握しました"),
        "Cline narration must show in the SubAgent viewer: {:?}",
        live.visible_thinking
    );
    assert!(
        live.hidden_text.is_empty(),
        "prose must not sit in hidden text_delta: {:?}",
        live.hidden_text
    );

    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("elapsed keepalive");
    live.ingest_available(&mut receiver);
    assert!(
        live.visible_thinking.contains("型と配信パスを把握しました"),
        "silence after prose must keep the open thought live: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.visible_thinking.contains("still working"),
        "elapsed ticks must not paint Thought-for chrome: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.hidden_text.contains('\u{200b}'),
        "SubAgent keepalive must not hide in text_delta: {:?}",
        live.hidden_text
    );

    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.stop_reason, "end_turn");
    assert!(
        segment.blocks.iter().any(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("型と配信パスを把握しました"))
        }),
        "answer must flush at end_turn: {:?}",
        segment.blocks
    );
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| !committed_progress_text(block).contains("still working")),
        "elapsed keepalive must not stay in the transcript: {:?}",
        segment.blocks
    );
    drop(sender);
    let _ = collect_frames(&mut receiver).await;
}

#[tokio::test]
async fn subagent_silence_keepalive_paints_elapsed_progress_not_blank_viewer() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"bash-ctx",
                "tool":"Bash",
                "title":"ctx search finish-prediction-inputs-cache",
                "arguments":{"command":"ctx search finish-prediction-inputs-cache"}
            }}),
            Some(&sender),
        )
        .await
        .expect("tool start");
    live.ingest_available(&mut receiver);
    assert!(
        !live.visible_server_tools.is_empty(),
        "Bash SubAgent must paint display-only server_tool_use first: {:?}",
        live.visible_server_tools
    );

    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("first elapsed");
    builder.age_turn_for_test(Duration::from_secs(8));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("second elapsed");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains('▶'),
        "long tool silence must reopen ▶ thinking after the server card: {:?}",
        live.visible_thinking
    );
    assert!(
        live.visible_thinking.contains('·')
            && live.visible_thinking.contains('s'),
        "keepalive must advance a visible elapsed clock: {:?}",
        live.visible_thinking
    );
    let tip = live
        .visible_thinking
        .chars()
        .filter(|ch| *ch != '\u{200b}')
        .collect::<String>();
    assert!(
        tip.trim_end()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| line.contains('▶') && line.contains('·')),
        "keepalive must re-tip ▶ with elapsed clock so CC does not flash blank Perambulating: {tip:?}"
    );
    assert!(
        !live.visible_thinking.contains("still working")
            && !live.visible_thinking.contains("last:"),
        "elapsed ticks must not paint Thought-for chrome: {:?}",
        live.visible_thinking
    );
    assert!(
        live.hidden_text.is_empty(),
        "elapsed ticks must not use hidden text_delta: {:?}",
        live.hidden_text
    );
    drop(sender);
    let _ = collect_frames(&mut receiver).await;
}

#[tokio::test]
async fn subagent_keepalive_without_tools_paints_thinking_tip() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("silent keepalive");
    live.ingest_available(&mut receiver);
    assert!(
        live.visible_thinking.contains("▶ Thinking")
            && live.visible_thinking.contains('·'),
        "tool-less silence must reopen with Thinking tip + clock, not blank ZWSP: {:?}",
        live.visible_thinking
    );
    drop(sender);
    let _ = collect_frames(&mut receiver).await;
}

#[tokio::test]
async fn subagent_keepalive_reopens_thinking_after_tool_use_closes_it() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"read-claude-md",
                "tool":"Read",
                "title":"Read scripts/CLAUDE.md",
                "arguments":{"path":"scripts/CLAUDE.md"}
            }}),
            Some(&sender),
        )
        .await
        .expect("tool start");
    live.ingest_available(&mut receiver);
    builder
        .thinking
        .close(&mut builder.blocks, Some(&sender))
        .await
        .expect("Codex tool_use closes thinking");
    live.ingest_available(&mut receiver);
    live.visible_thinking.clear();
    assert!(
        !builder.thinking.is_open(),
        "precondition: native tool_use left the thought closed"
    );

    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("reopen after silence");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains('▶')
            && live.visible_thinking.contains("Read scripts/CLAUDE.md"),
        "closed thought after Read must reopen with last ▶ progress: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.visible_thinking.contains("still working")
            && !live.visible_thinking.contains("Thought for"),
        "reopen must not restore Thought-for chrome: {:?}",
        live.visible_thinking
    );
    drop(sender);
    let _ = collect_frames(&mut receiver).await;
}

#[tokio::test]
async fn counts_only_validated_provider_web_evidence_once() {
    let mut builder = SegmentBuilder::new(1);
    let evidence = json!({
        "provider":"acp",
        "provenance":"provider-native-tool-completion",
        "kind":"web_search",
        "evidence_class":"search_result_only",
        "status":"completed",
        "verified":true,
        "result_summary":"provider search result",
        "source_urls":["https://example.com/result"]
    });
    send_evidence_updates(&mut builder, evidence).await;
    let segment = builder.finish(None).await.expect("segment");
    assert_eq!(segment.usage.web_search_requests, 1);
}


type LiveView = super::super::subagent_live_view::SubAgentLiveView;
type FrameTx = mpsc::Sender<Result<Bytes, Infallible>>;
type FrameRx = mpsc::Receiver<Result<Bytes, Infallible>>;

async fn model_delta(builder: &mut SegmentBuilder, sender: &FrameTx, item_id: &str, delta: &str) {
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{"itemId":item_id,"delta":delta}
            }),
            Some(sender),
        )
        .await
        .expect("model delta");
}

async fn reasoning_delta(
    builder: &mut SegmentBuilder,
    sender: &FrameTx,
    item_id: &str,
    summary_index: u64,
    delta: &str,
) {
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{
                    "itemId":item_id,
                    "summaryIndex":summary_index,
                    "delta":delta
                }
            }),
            Some(sender),
        )
        .await
        .expect("reasoning delta");
}

async fn drain_sse(receiver: &mut FrameRx) -> String {
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    sse
}

fn assert_no_noise(haystack: &str, noises: &[&str], label: &str) {
    for noise in noises {
        assert!(
            !haystack.contains(noise),
            "{label} still has `{noise}`: {haystack}"
        );
    }
}

fn assert_qwen_status_live(live: &LiveView) {
    assert!(live.turn_still_open(), "status chunk must not end the SubAgent turn");
    assert!(
        live.visible_thinking.contains("Starting inspection"),
        "SubAgent prose must paint thinking chrome live: {:?}",
        live.visible_thinking
    );
    assert!(
        live.hidden_text.is_empty(),
        "prose must not wait in hidden text_delta: {:?}",
        live.hidden_text
    );
    assert!(
        !live.visible_thinking.contains('▶'),
        "no tool chrome yet: {:?}",
        live.visible_thinking
    );
}

fn assert_qwen_read_live(live: &LiveView) {
    assert!(live.turn_still_open(), "tool start is mid-turn");
    assert!(
        live.visible_thinking.contains("▶ ReadFile")
            || live.visible_thinking.contains("▶ Read")
            || live
                .visible_server_tools
                .iter()
                .any(|name| name.contains("text_editor") || name.contains("bash")),
        "Read must paint server card or ▶ before finish: thinking={:?} server_tools={:?}",
        live.visible_thinking,
        live.visible_server_tools
    );
    assert!(
        !live.hidden_text.contains('▶'),
        "▶ must not wait in hidden text: {:?}",
        live.hidden_text
    );
}

fn text_has_starting_inspection(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("Starting inspection") && !text.contains("Status:"))
}

fn block_not_server_tool_use(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) != Some("server_tool_use")
}

fn block_text_omits(block: &Value, needle: &str) -> bool {
    !block
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains(needle))
}

async fn run_qwen_mid_turn_events(
    builder: &mut SegmentBuilder,
    sender: &FrameTx,
    receiver: &mut FrameRx,
    live: &mut LiveView,
) {
    model_delta(
        builder,
        sender,
        "qwen:message",
        "Starting inspection. Current action: reading CLAUDE.md.\n",
    )
    .await;
    live.ingest_available(receiver);
    assert_qwen_status_live(live);

    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"qwen-read-1",
                "tool":"Read",
                "title":"ReadFile: CLAUDE.md",
                "arguments":{"file_path":"/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/CLAUDE.md"}
            }}),
            Some(sender),
        )
        .await
        .expect("qwen read start");
    live.ingest_available(receiver);
    assert_qwen_read_live(live);

    builder
        .provider_tool_update(
            &json!({"params":{
                "callId":"qwen-read-1",
                "status":"completed",
                "title":"ReadFile: CLAUDE.md"
            }}),
            Some(sender),
        )
        .await
        .expect("qwen read complete");
    live.ingest_available(receiver);
    assert!(live.turn_still_open(), "completion is still mid-turn");
    assert!(
        live.visible_thinking.contains('✓')
            || live
                .visible_server_tools
                .iter()
                .any(|name| name.contains("text_editor") || name.contains("bash")),
        "completion must paint ✓ or keep the server card visible: thinking={:?} server_tools={:?}",
        live.visible_thinking,
        live.visible_server_tools
    );

    model_delta(
        builder,
        sender,
        "qwen:message",
        "Status: CLAUDE.md read. Current action: reading create-symlinks.sh.\n",
    )
    .await;
    live.ingest_available(receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains("Status: CLAUDE.md read"),
        "{:?}",
        live.visible_thinking
    );
}

async fn run_cc_bash_paint(
    builder: &mut SegmentBuilder,
    sender: &FrameTx,
    receiver: &mut FrameRx,
    live: &mut LiveView,
) {
    model_delta(
        builder,
        sender,
        "command-code:message",
        "Printing date, pausing 45 seconds, then printing TOKEN_QUALITY_BASH.\n",
    )
    .await;
    live.ingest_available(receiver);
    assert!(
        live.visible_thinking
            .contains("Printing date, pausing 45 seconds"),
        "CC narration before a card must stay in thinking, not vanish after Thought: {:?}",
        live.visible_thinking
    );
    assert!(
        live.hidden_text.is_empty(),
        "CC narration must not sit in hidden text_delta: {:?}",
        live.hidden_text
    );
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"cc-bash",
                    "tool":"Bash",
                    "title":"Bash",
                    "arguments":{"command":"bunx wrangler tail sync-realtime-data --format json"}
                }
            }),
            Some(sender),
        )
        .await
        .expect("cc bash start");
    live.ingest_available(receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_server_tools
            .iter()
            .any(|name| name == "bash_code_execution")
            || (live.visible_thinking.contains("▶ Bash")
                && live.visible_thinking.contains("wrangler tail")),
        "CC Bash must paint a display card or ▶ thinking: thinking={:?} server_tools={:?}",
        live.visible_thinking,
        live.visible_server_tools
    );
    assert!(
        !live.hidden_text.contains('▶'),
        "▶ must not sit in hidden text_delta: {:?}",
        live.hidden_text
    );
    builder
        .provider_tool_update(
            &json!({"params":{"callId":"cc-bash","status":"completed","title":"Bash"}}),
            Some(sender),
        )
        .await
        .expect("cc bash done");
    builder
        .activity_keepalive(Some(sender))
        .await
        .expect("cc elapsed");
    live.ingest_available(receiver);
    assert!(
        live.visible_thinking.contains('✓') || live.visible_thinking.contains("▶ Bash"),
        "CC Bash completion/elapsed must stay visible: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.hidden_text.contains('▶') && !live.hidden_text.contains("still working"),
        "CC must not dump tool chrome as text: {:?}",
        live.hidden_text
    );
}

async fn run_wrangler_dump_events(
    builder: &mut SegmentBuilder,
    sender: &FrameTx,
    dump: &str,
) {
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"cc-tail",
                    "tool":"Bash",
                    "title":"Bash",
                    "arguments":{"command":"bunx wrangler tail sync-realtime-data --format json"}
                }
            }),
            Some(sender),
        )
        .await
        .expect("bash");
    reasoning_delta(
        builder,
        sender,
        "cc:reasoning",
        0,
        "The wrangler tail JSON output appears to contain critical information.\n",
    )
    .await;
    reasoning_delta(builder, sender, "cc:reasoning", 1, dump).await;
    reasoning_delta(builder, sender, "cc:reasoning", 1, dump).await;
}

async fn run_canned_filler_events(builder: &mut SegmentBuilder, sender: &FrameTx) {
    for (method, item_id, delta) in [
        (
            "item/reasoning/summaryTextDelta",
            "cursor:reasoning",
            "Working on your request — I'll gather what I need and put together the result.\n",
        ),
        (
            "item/agentMessage/delta",
            "cursor:message",
            "I’ll audit the local ctx index and pull the evidence needed for the report.\n",
        ),
        (
            "item/agentMessage/delta",
            "cursor:message",
            "Continuing with the next step in the plan.\n",
        ),
        (
            "item/agentMessage/delta",
            "cursor:message",
            "Gathering the records for your report — running ctx queries and pulling the provenance.\n",
        ),
        (
            "item/reasoning/summaryTextDelta",
            "cursor:reasoning",
            "Thought for 17s\n",
        ),
    ] {
        builder
            .model_output_event(
                &json!({"method":method,"params":{"itemId":item_id,"summaryIndex":0,"delta":delta}}),
                Some(sender),
            )
            .await
            .expect("canned filler");
    }
}

fn thinking_text(builder: &SegmentBuilder) -> String {
    builder
        .blocks
        .iter()
        .filter_map(|block| block.get("thinking").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

fn committed_progress_text(block: &Value) -> String {
    ["text", "thinking"]
        .into_iter()
        .filter_map(|key| block.get(key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

async fn collect_frames(
    receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>,
) -> (usize, String) {
    let mut output = String::new();
    let mut frame_count = 0;
    while let Some(frame) = receiver.recv().await {
        frame_count += 1;
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    (frame_count, output)
}

async fn send_update_statuses(builder: &mut SegmentBuilder) {
    for status in ["pending", "in_progress"] {
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":"update-only",
                    "status":status,
                    "title":"WebFetch"
                }}),
                None,
            )
            .await
            .expect("provider update");
    }
}

async fn send_evidence_updates(builder: &mut SegmentBuilder, evidence: Value) {
    for (call_id, evidence) in [
        ("model-prose", json!("https://model.example/prose-url")),
        (
            "missing-source",
            json!({
                "provider":"acp",
                "provenance":"provider-native-tool-completion",
                "kind":"web_search",
                "evidence_class":"search_result_only",
                "status":"completed",
                "verified":true,
                "result_summary":"missing source URL",
                "source_urls":[]
            }),
        ),
        ("native-search", evidence.clone()),
        ("native-search", evidence),
    ] {
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":call_id,
                    "status":"completed",
                    "evidence":evidence
                }}),
                None,
            )
            .await
            .expect("provider update");
    }
}

#[test]
fn accepts_only_complete_native_web_evidence_with_a_valid_source() {
    let valid = json!({
        "provider":"acp",
        "provenance":"provider-native-tool-completion",
        "kind":"web_search",
        "evidence_class":"search_result_only",
        "status":"completed",
        "verified":true,
        "result_summary":"provider result",
        "source_urls":["https://example.com/result"]
    });
    assert!(validated_provider_web_evidence(Some(&valid)));
    assert!(!validated_provider_web_evidence(None));

    let mut invalid = valid.clone();
    invalid["provenance"] = json!("model-prose");
    assert!(!validated_provider_web_evidence(Some(&invalid)));

    let mut fetch = valid.clone();
    fetch["kind"] = json!("web_fetch");
    fetch["evidence_class"] = json!("fetch_verified");
    fetch["source_urls"] = json!(["http://example.com/fetch"]);
    assert!(validated_provider_web_evidence(Some(&fetch)));

    for (field, value) in [
        ("evidence_class", json!("fetch_verified")),
        ("status", json!("in_progress")),
        ("verified", json!(false)),
        ("result_summary", json!("  ")),
        ("source_urls", json!(["ftp://example.com/result"])),
        ("source_urls", json!([null])),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(!validated_provider_web_evidence(Some(&invalid)));
    }
}

#[tokio::test]
async fn command_code_bash_paints_thinking_progress_like_other_acp() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(true);
    let mut live = LiveView::default();
    run_cc_bash_paint(&mut builder, &sender, &mut receiver, &mut live).await;
    drop(sender);
}

#[tokio::test]
async fn subagent_keeps_only_latest_status_line_mid_turn() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(true);
    let mut live = super::super::subagent_live_view::SubAgentLiveView::default();
    for status in [
        "Status: inspecting wrangler config. No edits.",
        "Status: starting live wrangler tail.",
        "Status: after tail. Cause is catalog 502 r2_sql_unavailable.",
    ] {
        builder
            .model_output_event(
                &json!({
                    "method":"item/agentMessage/delta",
                    "params":{"itemId":"command-code:message","delta":status}
                }),
                Some(&sender),
            )
            .await
            .expect("status");
    }
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{
                    "itemId":"command-code:message",
                    "delta":"Status: prior tails show RS queue.Status: parsing those records.Status: before long-running wrangler tail (~55s)."
                }
            }),
            Some(&sender),
        )
        .await
        .expect("concat status");
    live.ingest_available(&mut receiver);
    assert!(
        live.visible_thinking
            .contains("before long-running wrangler tail"),
        "latest Status must remain visible mid-turn: {:?}",
        live.visible_thinking
    );
    assert!(
        live.hidden_text.is_empty(),
        "Status chrome must not dump into hidden text_delta: {:?}",
        live.hidden_text
    );
    assert!(
        builder.pending_answer.is_empty(),
        "Status chrome must not become the final answer: {:?}",
        builder.pending_answer
    );
    let segment = builder.finish(None).await.expect("finish");
    assert!(
        segment
            .blocks
            .iter()
            .all(|block| block_text_omits(block, "Status:")),
        "Status lines must not remain in the transcript: {:?}",
        segment.blocks
    );
    drop(sender);
}

#[tokio::test]
async fn subagent_omits_wrangler_json_dump_after_thought() {
    let dump = format!(
        "{{\"type\":\"exception\",\"outcome\":\"exception\",\"exceptions\":[{}]}}",
        r#"{"name":"TypeError","message":"runningStyle"},"#.repeat(40)
    );
    assert!(dump.len() > 200);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(true);
    let mut live = LiveView::default();
    run_wrangler_dump_events(&mut builder, &sender, &dump).await;
    live.ingest_available(&mut receiver);
    assert!(
        live.visible_thinking
            .contains("The wrangler tail JSON output appears to contain critical"),
        "short thought must remain: {:?}",
        live.visible_thinking
    );
    assert!(
        live.visible_thinking.contains("large tool output omitted")
            || live.visible_thinking.contains("still working")
            || live.visible_thinking.contains('▶'),
        "after Thought, viewer must show a short status not a blank panel: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.visible_thinking.contains("TypeError")
            && !live.hidden_text.contains("TypeError")
            && !live.hidden_text.contains("runningStyle"),
        "wrangler JSON must not be synced into the live viewer: thinking={:?} text={:?}",
        live.visible_thinking,
        live.hidden_text
    );
    drop(sender);
}

#[tokio::test]
async fn command_code_web_search_stays_on_native_thinking_not_server_tool_use() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(true);
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"cc-search",
                    "tool":"web_search",
                    "title":"web_search",
                    "arguments":{"query":"名古屋 天気"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("cc web_search");
    builder
        .model_output_event(
            &json!({
                "method":"item/agentMessage/delta",
                "params":{
                    "itemId":"command-code:message",
                    "delta":"名古屋は晴れ、最高35℃です。\n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("cc answer");
    drop(sender);
    let sse = drain_sse(&mut receiver).await;
    assert!(
        sse.contains("thinking_delta")
            && (sse.contains("▶ web_search") || sse.contains("▶ 名古屋")),
        "CC web_search must stay on native thinking ▶: {sse}"
    );
    assert!(
        sse.contains("名古屋 天気") || sse.contains("名古屋は晴れ"),
        "query or answer must stream in thinking chrome: {sse}"
    );
    assert!(
        !sse.contains("\"type\":\"server_tool_use\""),
        "Command Code must not close thinking for server_tool_use: {sse}"
    );
    assert!(
        !sse.contains("\"type\":\"tool_use\""),
        "must not emit executable tool_use: {sse}"
    );
    assert!(
        sse.matches("\"type\":\"signature_delta\"").count() <= 1,
        "thinking must stay open mid-turn (no Thought-for flicker): {sse}"
    );
    let segment = builder.finish(None).await.expect("finish");
    assert_eq!(segment.stop_reason, "end_turn");
    assert!(
        segment.blocks.iter().all(block_not_server_tool_use),
        "committed segment must not keep server_tool_use: {:?}",
        segment.blocks
    );
}

#[tokio::test]
async fn cursor_canned_thought_for_filler_is_dropped_from_subagent_viewer() {
    // Live TUI dump: repeating `Thought for Xs` + Cursor ctx filler around
    // Agent(Inspect AzooKey Rust tests).
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = LiveView::default();
    run_canned_filler_events(&mut builder, &sender).await;
    builder
        .provider_tool_call(
            &json!({"params":{
                "callId":"read-azoo",
                "tool":"Read",
                "title":"Read AzooKey tests",
                "arguments":{"path":"AzooKeyTests.swift"}
            }}),
            Some(&sender),
        )
        .await
        .expect("real tool");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("elapsed");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains("▶ Read"),
        "real tool progress must stay visible: {:?}",
        live.visible_thinking
    );
    let noises = [
        "Working on your request",
        "I'll gather what I need",
        "I’ll gather what I need",
        "audit the local ctx",
        "Continuing with the next step",
        "Gathering the records for your report",
        "Thought for",
        "still working",
    ];
    assert_no_noise(&live.visible_thinking, &noises, "thinking");
    assert_no_noise(&live.hidden_text, &noises, "hidden text");
    assert_eq!(
        builder
            .blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .count(),
        1,
        "canned filler must not open extra thinking blocks: {:?}",
        builder.blocks
    );
    let segment = builder.finish(None).await.expect("finish");
    let transcript = serde_json::to_string(&segment.blocks).expect("transcript json");
    assert_no_noise(
        &transcript,
        &[
            "Working on your request",
            "audit the local ctx",
            "Continuing with the next step",
            "Thought for",
            "still working",
        ],
        "transcript",
    );
    drop(sender);
    let _ = collect_frames(&mut receiver).await;
}

#[tokio::test]
async fn cline_subagent_read_paints_display_only_server_tool_use() {
    // High-effort SubAgents collapse ▶ thinking to Wandering; paint a
    // display-only server card so mid-turn Read is visible like advisor Bash.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"cline-read",
                    "tool":"Read",
                    "title":"Read File",
                    "arguments":{"path":"/Users/kkk4oru/ghq/github.com/kkkaoru/horse-racing-data/apps/local-postgresql/CLAUDE.md"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("cline read");
    drop(sender);
    let sse = drain_sse(&mut receiver).await;
    assert!(
        sse.contains("\"type\":\"server_tool_use\"")
            && sse.contains("text_editor_code_execution"),
        "ACP SubAgent Read must paint display-only server_tool_use: {sse}"
    );
    assert!(
        !sse.contains("\"type\":\"tool_use\""),
        "must not emit executable tool_use: {sse}"
    );
    let segment = builder.finish(None).await.expect("finish");
    assert!(
        segment.blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("server_tool_use")
        }),
        "committed segment keeps server_tool_use chrome: {:?}",
        segment.blocks
    );
}

#[tokio::test]
async fn main_session_acp_read_does_not_emit_server_tool_use() {
    // Old non-subagent path: thinking ▶ only. Main session can show text.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    builder
        .provider_tool_call(
            &json!({
                "params":{
                    "callId":"main-read",
                    "tool":"Read",
                    "title":"Read File",
                    "arguments":{"path":"CLAUDE.md"}
                }
            }),
            Some(&sender),
        )
        .await
        .expect("main read");
    drop(sender);
    let sse = drain_sse(&mut receiver).await;
    assert!(
        !sse.contains("\"type\":\"server_tool_use\""),
        "main session must not paint server_tool_use: {sse}"
    );
    assert!(
        sse.contains("thinking_delta") && sse.contains("▶ Read"),
        "main session still uses thinking ▶: {sse}"
    );
}
