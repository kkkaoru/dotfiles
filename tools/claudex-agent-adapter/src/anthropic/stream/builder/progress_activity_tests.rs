use std::{convert::Infallible, time::Duration};

use axum::body::Bytes;
use serde_json::json;
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
async fn elapsed_tip_closes_zwsp_prime_when_external_tools_left_thinking_open() {
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_primed_thinking();
    builder.external_tool_calls = 1;
    builder
        .provider_tool_calls
        .push(("call-1".to_owned(), "Read".to_owned()));
    builder.age_turn_for_test(Duration::from_secs(5));
    assert!(builder.thinking.is_open());
    builder
        .activity_keepalive(None)
        .await
        .expect("keepalive with external tools and zwsp prime");
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
