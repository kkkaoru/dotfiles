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
