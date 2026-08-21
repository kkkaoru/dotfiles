use std::{convert::Infallible, time::Duration};

use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::super::SegmentBuilder;
use crate::anthropic::stream::subagent_live_view::SubAgentLiveView;

fn grok_reasoning_delta(text: &str) -> serde_json::Value {
    json!({
        "method":"item/reasoning/summaryTextDelta",
        "params":{
            "itemId":"grok:reasoning",
            "summaryIndex":0,
            "delta":text
        }
    })
}

fn thinking_has_nested_elapsed_chrome(text: &str) -> bool {
    text.contains("▶ Thinking") && (text.contains("· 0s") || text.contains("· 1s"))
}

#[tokio::test]
async fn grok_reasoning_keepalive_does_not_nest_thinking_chrome() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = SubAgentLiveView::default();
    builder
        .model_output_event(
            &grok_reasoning_delta("Anchor the Avita research next.\n"),
            Some(&sender),
        )
        .await
        .expect("grok thought");
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive 0s");
    builder.age_turn_for_test(Duration::from_secs(1));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive 1s");
    live.ingest_available(&mut receiver);
    assert!(
        live.visible_thinking.contains("Anchor the Avita"),
        "Grok CoT must stay on thinking: {:?}",
        live.visible_thinking
    );
    assert!(
        !thinking_has_nested_elapsed_chrome(&live.visible_thinking),
        "Thinking for Ns must not wrap ▶ Thinking chrome: {:?}",
        live.visible_thinking
    );
    drop(sender);
}

#[tokio::test]
async fn toolless_keepalive_does_not_stack_thinking_elapsed_tips() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    let mut live = SubAgentLiveView::default();
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive 0s");
    builder.age_turn_for_test(Duration::from_secs(1));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive 1s");
    live.ingest_available(&mut receiver);
    let text = &live.visible_thinking;
    assert!(
        !(text.contains("▶ Thinking… · 0s") && text.contains("▶ Thinking… · 1s")),
        "stacked nested thinking chrome: {text:?}"
    );
    assert!(
        !thinking_has_nested_elapsed_chrome(text),
        "tool-less silence must not paint ▶ Thinking inside thinking: {text:?}"
    );
    drop(sender);
}

#[tokio::test]
async fn elapsed_tip_returns_when_external_tools_ran_without_open_thinking() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder.external_tool_calls = 1;
    builder
        .provider_tool_calls
        .push(("call-1".to_owned(), "Read".to_owned()));
    builder.age_turn_for_test(Duration::from_secs(5));
    builder
        .activity_keepalive(None)
        .await
        .expect("keepalive with external tools and closed thinking");
    assert!(
        !builder.thinking.is_open(),
        "closed thinking plus external tools must not open a tip block"
    );
}

#[tokio::test]
async fn elapsed_tip_does_not_reopen_synthetic_prime_with_external_tools() {
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_primed_thinking();
    builder.external_tool_calls = 1;
    builder
        .provider_tool_calls
        .push(("call-1".to_owned(), "Read".to_owned()));
    builder.age_turn_for_test(Duration::from_secs(5));
    builder
        .activity_keepalive(None)
        .await
        .expect("keepalive with external tools and silent prime");
    assert!(
        !builder.thinking.is_open() && builder.blocks.is_empty(),
        "silent prime and elapsed keepalive must stay out of the transcript: {:?}",
        builder.blocks
    );
}

#[tokio::test]
async fn main_live_cot_keepalive_continues_after_external_tool() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .model_output_event(
            &grok_reasoning_delta("Check the provider response before continuing.\n"),
            Some(&sender),
        )
        .await
        .expect("main live CoT");
    let before = builder.blocks[0]["thinking"].clone();
    builder.external_tool_calls = 1;
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("keepalive after external tool with live CoT");

    assert_eq!(
        builder.blocks[0]["thinking"], before,
        "heartbeat is stream-only"
    );
    drop(sender);
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        sse.contains("thinking_delta"),
        "live CoT must keep streaming provider reasoning: {sse}"
    );
    assert!(
        !sse.contains('\u{200b}'),
        "activity keepalive must not add synthetic zero-width reasoning: {sse}"
    );
}

#[tokio::test]
async fn subagent_visible_progress_without_last_tool_keeps_elapsed_heartbeat() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .stream_progress_text("▶ Inspecting the provider response\n", Some(&sender))
        .await
        .expect("visible subagent progress");
    let before = builder.blocks[0]["thinking"].clone();
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("elapsed heartbeat without a last tool");

    assert_eq!(
        builder.blocks[0]["thinking"], before,
        "elapsed keepalive must not overwrite visible progress"
    );
    drop(sender);
    let mut sse = String::new();
    while let Some(frame) = receiver.recv().await {
        sse.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    assert!(
        sse.contains("thinking_delta"),
        "visible progress must keep streaming provider progress: {sse}"
    );
    assert!(
        !sse.contains('\u{200b}'),
        "activity keepalive must not add synthetic zero-width progress: {sse}"
    );
}

#[tokio::test]
async fn subagent_visible_progress_with_last_tool_stays_unchanged_on_keepalive() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .stream_progress_text("▶ Inspecting the provider response\n", None)
        .await
        .expect("visible subagent progress");
    builder
        .provider_tool_calls
        .push(("read-1".to_owned(), "Read".to_owned()));
    builder.age_turn_for_test(Duration::from_secs(5));
    builder
        .activity_keepalive(None)
        .await
        .expect("elapsed tip after visible progress");

    assert!(
        builder.blocks[0]["thinking"]
            .as_str()
            .is_some_and(|text| text == "▶ Inspecting the provider response\n"),
        "elapsed keepalive must not rewrite visible ACP progress: {:?}",
        builder.blocks
    );
}

#[tokio::test]
async fn collapsed_launch_prime_closes_before_visible_subagent_progress() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .thinking
        .progress_status_keep_open(&mut builder.blocks, "SubAgent starting\n", None)
        .await
        .expect("collapsed launch prime");
    builder
        .stream_progress_text("visible status\n", None)
        .await
        .expect("visible progress after prime");
}

#[tokio::test]
async fn subagent_raw_reasoning_skips_missing_and_whitespace_deltas() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .raw_reasoning_delta(&json!({"params":{"itemId":"r1"}}), None)
        .await
        .expect("missing delta");
    builder
        .raw_reasoning_delta(&json!({"params":{"itemId":"r1","delta":"  \n  "}}), None)
        .await
        .expect("whitespace delta");
    assert!(builder.pending_reasoning.is_empty());
}

fn committed_thinking(blocks: &[serde_json::Value]) -> Vec<String> {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("thinking"))
        .map(|block| {
            block
                .get("thinking")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn t1_prime_and_keepalive_only_does_not_commit_empty_thinking() {
    let mut builder = SegmentBuilder::new(1).with_primed_thinking();
    builder
        .activity_keepalive(None)
        .await
        .expect("silence keepalive");
    let segment = builder.finish(None).await.expect("finish");
    assert!(
        committed_thinking(&segment.blocks).is_empty(),
        "prime+keepalive must not leave empty/STATUS thinking: {:?}",
        segment.blocks
    );
    let dumped = serde_json::to_string(&segment.blocks).expect("json");
    assert!(!dumped.contains('\u{200b}'), "{dumped}");
    assert!(!dumped.contains("Claudex is still working"), "{dumped}");
    assert!(!dumped.contains("Thought for"), "{dumped}");
}

#[tokio::test]
async fn t1_main_does_not_reopen_thinking_after_parent_agent_tool_use() {
    let mut builder = SegmentBuilder::new(1).with_primed_thinking();
    builder.external_tool_calls = 1;
    builder.thinking.close(&mut builder.blocks, None).await.ok();
    builder
        .activity_keepalive(None)
        .await
        .expect("keepalive after parent Agent");
    assert!(
        !builder.thinking.is_open(),
        "main must not reopen thinking keepalive after Agent/Task tool_use"
    );
    let segment = builder.finish(None).await.expect("finish");
    assert!(committed_thinking(&segment.blocks).is_empty());
}

#[tokio::test]
async fn t1_stop_without_cot_or_tip_drops_thinking_block() {
    let mut builder = SegmentBuilder::new(1).with_primed_thinking();
    let segment = builder.finish(None).await.expect("finish");
    assert!(committed_thinking(&segment.blocks).is_empty());
    let dumped = serde_json::to_string(&segment.blocks).expect("json");
    assert!(!dumped.contains('\u{200b}'), "{dumped}");
    assert!(!dumped.contains("Claudex is still working"), "{dumped}");
}

#[tokio::test]
async fn t14_turn_progress_stays_off_the_transcript() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .provider_tool_calls
        .push(("read-1".to_owned(), "Read".to_owned()));
    builder
        .provider_tool_calls
        .push(("bash-1".to_owned(), "Bash".to_owned()));
    builder
        .provider_tool_terminal_ids
        .insert("read-1".to_owned());
    builder
        .provider_tool_terminal_ids
        .insert("bash-1".to_owned());
    let _ = builder.finish(None).await.expect("finish");
    let progress = builder.last_turn_progress.clone();
    assert_eq!(progress.len(), 2, "{progress:?}");
    assert_eq!(progress[0].title, "Read");
    assert_eq!(progress[1].title, "Bash");
    assert_eq!(progress[0].status, "completed");
    assert!(
        progress.iter().all(|event| event.elapsed_ms < u64::MAX),
        "{progress:?}"
    );
}

#[derive(Default)]
struct ClineSseClient {
    events: Vec<Value>,
    blocks: Vec<(usize, Value)>,
    open_block: Option<usize>,
    message_start_count: usize,
    message_delta_count: usize,
    message_stop_count: usize,
}

impl ClineSseClient {
    fn ingest(&mut self, frame: &Bytes) {
        let frame = String::from_utf8(frame.to_vec()).expect("UTF-8 SSE");
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE data");
        let event: Value = serde_json::from_str(data).expect("SSE JSON");
        self.events.push(event.clone());

        match event["type"].as_str().expect("SSE event type") {
            "message_start" => {
                assert_eq!(self.open_block, None, "message must start without a block");
                self.message_start_count += 1;
            }
            "content_block_start" => {
                let index = event["index"].as_u64().expect("content start index") as usize;
                assert!(
                    self.open_block.replace(index).is_none(),
                    "nested content block"
                );
                self.blocks.push((index, event["content_block"].clone()));
            }
            "content_block_delta" => self.ingest_content_block_delta(&event),
            "content_block_stop" => {
                let index = event["index"].as_u64().expect("content stop index") as usize;
                assert_eq!(
                    self.open_block.take(),
                    Some(index),
                    "stop without its start"
                );
            }
            "message_delta" => {
                assert!(self.open_block.is_none(), "message delta before block stop");
                assert_eq!(
                    event["delta"]["stop_reason"].as_str(),
                    Some("end_turn"),
                    "blank Cline turn must finish with end_turn"
                );
                self.message_delta_count += 1;
            }
            "message_stop" => {
                assert!(self.open_block.is_none(), "message stop with open block");
                self.message_stop_count += 1;
            }
            other => panic!("unexpected SSE event type: {other}"),
        }
    }

    fn ingest_content_block_delta(&mut self, event: &Value) {
        let index = event["index"].as_u64().expect("content delta index") as usize;
        assert_eq!(self.open_block, Some(index), "delta without its open block");
        let block = self
            .blocks
            .iter_mut()
            .find(|(block_index, _)| *block_index == index)
            .map(|(_, block)| block)
            .expect("delta block");
        let delta = &event["delta"];
        match delta["type"].as_str().expect("content delta type") {
            "thinking_delta" => append_string(block, "thinking", delta["thinking"].as_str()),
            "signature_delta" => append_string(block, "signature", delta["signature"].as_str()),
            "text_delta" => append_string(block, "text", delta["text"].as_str()),
            "input_json_delta" => {}
            other => panic!("unexpected content delta type: {other}"),
        }
    }

    fn assert_clean_blank_turn(&self) {
        assert_eq!(self.message_start_count, 1, "exactly one message_start");
        assert_eq!(self.message_delta_count, 1, "exactly one message_delta");
        assert_eq!(self.message_stop_count, 1, "exactly one message_stop");
        assert_eq!(self.open_block, None, "all content blocks must be closed");
        assert_eq!(
            self.blocks.len(),
            1,
            "blank Cline turn must emit substitute text: {:?}",
            self.blocks
        );
        assert_eq!(self.blocks[0].0, 0);
        assert_eq!(self.blocks[0].1["type"], "text");
        assert_eq!(
            self.blocks[0].1["text"],
            "Provider completed with no assistant content. The route returned no assistant text or tools. This is a failure, not a completed result."
        );
        assert!(
            self.events.iter().all(|event| {
                event["type"] != "content_block_start"
                    || event["content_block"]["type"] != "thinking"
            }),
            "blank Cline turn must not start a thinking block: {:?}",
            self.events
        );
        assert!(
            self.events.iter().all(|event| {
                event["type"] != "content_block_delta" || event["delta"]["type"] != "thinking_delta"
            }),
            "blank Cline turn must not emit thinking deltas: {:?}",
            self.events
        );
        let wire = serde_json::to_string(&self.events).expect("event JSON");
        assert!(
            !wire.contains('\u{200b}'),
            "ZWSP leaked to the client: {wire}"
        );
        assert!(
            !wire.contains("claudex_activity_keepalive"),
            "keepalive signature leaked to the client: {wire}"
        );
        assert_eq!(
            self.events.last().and_then(|event| event["type"].as_str()),
            Some("message_stop"),
            "message_stop must be the terminal event"
        );
    }
}

fn append_string(block: &mut Value, key: &str, value: Option<&str>) {
    let Some(value) = value else {
        panic!("missing {key} content delta");
    };
    let current = block[key].as_str().unwrap_or_default().to_owned();
    block[key] = Value::String(format!("{current}{value}"));
}

#[tokio::test]
async fn cline_blank_thought_does_not_emit_ghost_thinking_on_wire_or_client() {
    use crate::anthropic::stream::{prepare::prime_subagent_sse, protocol::send_stream_completion};

    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    assert!(
        prime_subagent_sse(
            &sender,
            "cline-pass/deepseek-v4-flash",
            1,
            true,
            Some("xhigh"),
        ),
        "Cline prime must emit message_start"
    );

    let mut builder =
        SegmentBuilder::for_turn(1, true, "cline-pass/deepseek-v4-flash").with_primed_thinking();
    builder.age_turn_for_test(Duration::from_secs(2));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("first elapsed keepalive");
    builder.age_turn_for_test(Duration::from_secs(2));
    builder
        .activity_keepalive(Some(&sender))
        .await
        .expect("second elapsed keepalive");
    builder
        .model_output_event(
            &json!({
                "method":"item/reasoning/summaryTextDelta",
                "params":{
                    "itemId":"cline:reasoning",
                    "summaryIndex":0,
                    "delta":"\n\n  \n"
                }
            }),
            Some(&sender),
        )
        .await
        .expect("discard whitespace-only Cline thought");

    let segment = builder
        .finish(Some(&sender))
        .await
        .expect("terminal Cline finish");
    send_stream_completion(&sender, &segment).await;
    drop(sender);

    let mut client = ClineSseClient::default();
    while let Some(frame) = receiver.recv().await {
        client.ingest(&frame.expect("SSE frame"));
    }
    client.assert_clean_blank_turn();

    assert!(
        segment.blocks.iter().all(|block| {
            block.get("type").and_then(Value::as_str) != Some("thinking")
                && !block.to_string().contains('\u{200b}')
                && !block.to_string().contains("claudex_activity_keepalive")
        }),
        "local committed blocks must stay clean: {:?}",
        segment.blocks
    );
}
