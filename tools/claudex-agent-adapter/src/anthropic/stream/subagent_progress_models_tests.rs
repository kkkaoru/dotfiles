//! Live SubAgent viewer progress across provider models.
//!
//! Cline was the first blank-viewer report, but Cursor (nested under Spark),
//! Qwen, Grok, Copilot, Command Code, and Spark/Codex all hide AgentMessage as
//! `text_delta` until end_turn unless `is_subagent` mirrors it to thinking.

use axum::body::Bytes;
use serde_json::{Value, json};
use std::convert::Infallible;
use tokio::sync::mpsc;

use super::{builder::SegmentBuilder, subagent_live_view::SubAgentLiveView};

struct Tool {
    call_id: &'static str,
    name: &'static str,
    title: &'static str,
    arg_key: &'static str,
    arg_value: &'static str,
}

struct Case {
    name: &'static str,
    prose: Option<&'static str>,
    prose_item_id: &'static str,
    reasoning: Option<&'static str>,
    tool: Option<Tool>,
    expect_visible: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "cursor-acp",
        prose: Some(
            "ContextVar は worker スレッドに伝わらないので、スレッド共有のフラグに切り替えます。\n",
        ),
        prose_item_id: "cursor:message",
        reasoning: None,
        tool: Some(Tool {
            call_id: "cursor-read",
            name: "Read",
            title: "ReadFile: predict_container.py",
            arg_key: "path",
            arg_value: "apps/finish-position-predict-container/src/predict.py",
        }),
        expect_visible: &["ContextVar", "▶ Read"],
    },
    Case {
        name: "qwen-acp",
        prose: Some("Phase 1: reading CLAUDE.md.\n"),
        prose_item_id: "qwen:message",
        reasoning: None,
        tool: Some(Tool {
            call_id: "qwen-grep",
            name: "Grep",
            title: "Grep target_race",
            arg_key: "pattern",
            arg_value: "target_race",
        }),
        expect_visible: &["Phase 1", "▶ Grep"],
    },
    Case {
        name: "grok-acp",
        prose: None,
        prose_item_id: "grok:message",
        reasoning: Some("Plan the per-race cache seed next.\n"),
        tool: Some(Tool {
            call_id: "grok-bash",
            name: "Bash",
            title: "ls apps/finish-position-predict-container",
            arg_key: "command",
            arg_value: "ls apps/finish-position-predict-container",
        }),
        // CoT stays tip-only live (▶ Thinking); Bash title must stay visible.
        expect_visible: &["▶ Thinking", "▶ ls"],
    },
    Case {
        name: "copilot-acp",
        prose: Some("Inspecting the prediction pipeline next.\n"),
        prose_item_id: "copilot:message",
        reasoning: None,
        tool: Some(Tool {
            call_id: "copilot-read",
            name: "Read",
            title: "Read cache_seed.py",
            arg_key: "path",
            arg_value: "apps/finish-position-predict-container/src/cache_seed.py",
        }),
        expect_visible: &["Inspecting the prediction", "▶ Read"],
    },
    Case {
        name: "cline-acp",
        prose: Some(
            "型と配信パスを把握しました。既存 finish-prediction-inputs-cache を確認します。\n",
        ),
        prose_item_id: "cline:message",
        reasoning: Some("Check the cache seed path before editing.\n"),
        tool: None,
        expect_visible: &["型と配信パスを把握しました", "▶ Thinking"],
    },
    Case {
        name: "spark-codex",
        prose: Some("Seasoning per-race scope safety and cache seed behavior.\n"),
        prose_item_id: "spark:message",
        reasoning: Some("Trace filter_races_by_scope before editing.\n"),
        tool: None,
        expect_visible: &["▶ Thinking", "Seasoning per-race"],
    },
    Case {
        name: "command-code-acp",
        prose: Some("AVITA Inc. is an avatar company founded by Hiroshi Ishiguro.\n"),
        prose_item_id: "command-code:message",
        reasoning: Some("Check AVITA Inc. official site and filings.\n"),
        tool: Some(Tool {
            call_id: "cmd-search",
            name: "web_search",
            title: "web_search",
            arg_key: "query",
            arg_value: "AVITA株式会社",
        }),
        expect_visible: &["AVITA Inc. is an avatar", "▶ Thinking", "▶"],
    },
];

#[tokio::test]
async fn live_subagent_progress_is_visible_for_each_provider_model() {
    for case in CASES {
        run_case(case).await;
    }
}

#[tokio::test]
async fn cursor_prose_without_subagent_flag_stays_hidden_until_end_turn() {
    // Old live failure (fa522331 / claudex-cursor auto under spark):
    // AgentMessage stayed in hidden text_delta, so the nested Cursor viewer
    // was blank for minutes while tokens were still flowing.
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1);
    let mut live = SubAgentLiveView::default();
    builder
        .model_output_event(
            &agent_message("cursor:message", CASES[0].prose.unwrap()),
            Some(&sender),
        )
        .await
        .expect("cursor prose without subagent flag");
    live.ingest_available(&mut receiver);
    assert!(
        live.hidden_text.contains("ContextVar"),
        "{}: old path hid prose in text_delta: {:?}",
        CASES[0].name,
        live.hidden_text
    );
    assert!(
        !live.visible_thinking.contains("ContextVar"),
        "{}: without is_subagent the viewer stayed blank: {:?}",
        CASES[0].name,
        live.visible_thinking
    );
    drop(sender);
}

#[tokio::test]
async fn command_code_subagent_answer_streams_live_text_not_only_thinking() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(true);
    let mut live = SubAgentLiveView::default();

    builder
        .model_output_event(
            &agent_message(
                "command-code-abc:message",
                "● 検索中: AVITA株式会社。次: 公式サイト取得\n",
            ),
            Some(&sender),
        )
        .await
        .expect("command-code status");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        !live.hidden_text.contains("検索中: AVITA"),
        "● status chrome must not dump as text: {:?}",
        live.hidden_text
    );

    builder
        .model_output_event(
            &agent_message(
                "command-code-abc:message",
                "# AVITA株式会社\n設立: 2018年\n",
            ),
            Some(&sender),
        )
        .await
        .expect("command-code answer");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(
        live.visible_thinking.contains("設立: 2018年"),
        "CC answer before server_tool_use must stay visible after Thought, not hidden text: {:?}",
        live.visible_thinking
    );
    assert!(
        !live.hidden_text.contains("設立: 2018年"),
        "text_delta stays locked until server_tool_use: {:?}",
        live.hidden_text
    );
    let segment = builder.finish(None).await.expect("finish");
    assert!(
        segment.blocks.iter().any(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("設立: 2018年"))
        }),
        "answer must remain in transcript: {:?}",
        segment.blocks
    );
    assert!(
        segment.blocks.iter().all(|block| {
            !block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains('●') || text.contains("検索中"))
        }),
        "status chrome must be stripped from transcript: {:?}",
        segment.blocks
    );
    drop(sender);
}

async fn run_case(case: &Case) {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let command_code = case.name == "command-code-acp";
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_command_code_progress(command_code);
    feed_case_events(&mut builder, case, &sender).await;
    let mut live = SubAgentLiveView::default();
    live.ingest_available(&mut receiver);
    assert_live_progress(case, &live, command_code);
    assert_finished_transcript(case, &mut builder).await;
    drop(sender);
}

async fn feed_case_events(
    builder: &mut SegmentBuilder,
    case: &Case,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) {
    if let Some(reasoning) = case.reasoning {
        builder
            .model_output_event(&reasoning_delta(case.name, reasoning), Some(sender))
            .await
            .unwrap_or_else(|error| panic!("{} reasoning: {error}", case.name));
    }
    if let Some(prose) = case.prose {
        builder
            .model_output_event(&agent_message(case.prose_item_id, prose), Some(sender))
            .await
            .unwrap_or_else(|error| panic!("{} prose: {error}", case.name));
    }
    if let Some(tool) = &case.tool {
        builder
            .provider_tool_call(&provider_tool(tool), Some(sender))
            .await
            .unwrap_or_else(|error| panic!("{} tool: {error}", case.name));
    }
    builder
        .activity_keepalive(Some(sender))
        .await
        .unwrap_or_else(|error| panic!("{} keepalive: {error}", case.name));
}

fn assert_live_progress(case: &Case, live: &SubAgentLiveView, command_code: bool) {
    assert!(
        live.turn_still_open(),
        "{}: progress must stay mid-turn",
        case.name
    );
    assert!(
        live.hidden_text.is_empty()
            || (!live.hidden_text.contains('▶') && !live.hidden_text.contains("still working")),
        "{}: ▶/still-working must not dump as text: thinking={:?} text={:?}",
        case.name,
        live.visible_thinking,
        live.hidden_text
    );
    if command_code {
        assert!(
            !live.visible_thinking.contains("▶ Command Code"),
            "{}: keepalive must not dump ▶ Command Code chrome: {:?}",
            case.name,
            live.visible_thinking
        );
    } else {
        assert!(
            live.hidden_text.is_empty(),
            "{}: SubAgent prose/keepalive must not sit in hidden text_delta: {:?}",
            case.name,
            live.hidden_text
        );
    }
    for needle in case.expect_visible {
        assert!(
            live.visible_thinking.contains(needle)
                || live.hidden_text.contains(needle)
                || live
                    .visible_server_tools
                    .iter()
                    .any(|name| name.contains(needle)),
            "{}: missing `{needle}` in live viewer: thinking={:?} text={:?} server_tools={:?}",
            case.name,
            live.visible_thinking,
            live.hidden_text,
            live.visible_server_tools
        );
    }
    if case.tool.is_some() {
        assert!(
            live.visible_server_tools.is_empty(),
            "{}: ACP SubAgent tools stay on native thinking, not server_tool_use: thinking={:?} server_tools={:?}",
            case.name,
            live.visible_thinking,
            live.visible_server_tools
        );
        assert!(
            live.visible_thinking.contains('▶'),
            "{}: tool progress must stay in the open thinking block: {:?}",
            case.name,
            live.visible_thinking
        );
    }
}

async fn assert_finished_transcript(case: &Case, builder: &mut SegmentBuilder) {
    let Some(prose) = case.prose else {
        return;
    };
    let segment = builder.finish(None).await.expect(case.name);
    assert!(
        segment.blocks.iter().any(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(prose.trim()))
        }),
        "{}: answer prose must flush at end_turn: {:?}",
        case.name,
        segment.blocks
    );
    assert!(
        segment.blocks.iter().all(|block| {
            !block
                .get("thinking")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("still working"))
                && !block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("still working") || text.contains('▶'))
        }),
        "{}: live chrome must not remain in transcript: {:?}",
        case.name,
        segment.blocks
    );
}

fn agent_message(item_id: &str, delta: &str) -> Value {
    json!({
        "method":"item/agentMessage/delta",
        "params":{"itemId":item_id,"delta":delta}
    })
}

fn reasoning_delta(case: &str, delta: &str) -> Value {
    json!({
        "method":"item/reasoning/summaryTextDelta",
        "params":{"itemId":format!("{case}:reasoning"),"summaryIndex":0,"delta":delta}
    })
}

fn provider_tool(tool: &Tool) -> Value {
    json!({
        "params":{
            "callId":tool.call_id,
            "tool":tool.name,
            "title":tool.title,
            "arguments":{tool.arg_key:tool.arg_value}
        }
    })
}
