//! Live SubAgent viewer progress across provider models.
//!
//! ACP workers stream real CoT / compact prose plus ▶ tool chrome in native
//! thinking blocks. Pi/Grok stream CoT then native `tool_use` on
//! `toolcall_start` (not ACP `providerTool` ▶). Codex cases also stream CoT
//! then native `tool_use` (see `fugu_codex_*` in tests.rs). Canned filler is
//! still dropped.

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

enum ReasoningKind {
    /// ACP AgentThoughtChunk → `item/reasoning/summaryTextDelta`.
    Summary,
    /// Codex/GPT/GLM/Fugu raw CoT → `item/reasoning/textDelta`.
    RawTextDelta,
}

struct Case {
    name: &'static str,
    prose: Option<&'static str>,
    prose_item_id: &'static str,
    reasoning: Option<&'static str>,
    reasoning_kind: ReasoningKind,
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
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "cursor-read",
            name: "Read",
            title: "ReadFile: predict_container.py",
            arg_key: "path",
            arg_value: "apps/finish-position-predict-container/src/predict.py",
        }),
        // Compact prose + ▶ Read must remain visible in the native block.
        expect_visible: &["ContextVar", "▶ Read"],
    },
    Case {
        name: "auto-acp",
        prose: Some("Routing through Cursor auto for the nested SubAgent turn.\n"),
        prose_item_id: "auto:message",
        reasoning: Some("Prefer the sticky Cursor backend for this auto turn.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "auto-read",
            name: "Read",
            title: "Read CLAUDE.md",
            arg_key: "path",
            arg_value: "CLAUDE.md",
        }),
        expect_visible: &["Routing through Cursor", "Prefer the sticky", "▶ Read"],
    },
    Case {
        name: "qwen-acp",
        prose: Some("Phase 1: reading CLAUDE.md.\n"),
        prose_item_id: "qwen:message",
        reasoning: None,
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "qwen-grep",
            name: "Grep",
            title: "Grep target_race",
            arg_key: "pattern",
            arg_value: "target_race",
        }),
        expect_visible: &["Phase 1: reading", "▶ Grep"],
    },
    Case {
        name: "grok-acp",
        prose: None,
        prose_item_id: "grok:message",
        reasoning: Some("Plan the per-race cache seed next.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "grok-bash",
            name: "Bash",
            title: "ls apps/finish-position-predict-container",
            arg_key: "command",
            arg_value: "ls apps/finish-position-predict-container",
        }),
        // Reasoning body + Bash title (not tip-only ▶ Thinking).
        expect_visible: &["Plan the per-race cache seed", "▶ ls"],
    },
    Case {
        name: "grok-pi",
        prose: Some("Investigating the adapter stream path next.\n"),
        prose_item_id: "grok-pi:message",
        reasoning: Some("Open Read before arguments finish streaming.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "grok-pi-read",
            name: "Read",
            title: "Read CLAUDE.md",
            arg_key: "path",
            arg_value: "CLAUDE.md",
        }),
        expect_visible: &[
            "Investigating the adapter stream",
            "Open Read before arguments",
            "Read",
        ],
    },
    Case {
        name: "copilot-acp",
        prose: Some("Inspecting the prediction pipeline next.\n"),
        prose_item_id: "copilot:message",
        reasoning: None,
        reasoning_kind: ReasoningKind::Summary,
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
        reasoning_kind: ReasoningKind::Summary,
        tool: None,
        expect_visible: &["型と配信パスを把握しました", "Check the cache seed"],
    },
    Case {
        name: "deepseek-acp",
        // Denylisted for live spawn, but the ACP→thinking tip path must still work.
        prose: Some("DeepSeek is reading the cache seed module next.\n"),
        prose_item_id: "deepseek:message",
        reasoning: Some("Trace the DeepSeek flash path before editing.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "deepseek-read",
            name: "Read",
            title: "Read cache_seed.py",
            arg_key: "path",
            arg_value: "apps/finish-position-predict-container/src/cache_seed.py",
        }),
        expect_visible: &["DeepSeek is reading", "Trace the DeepSeek flash", "▶ Read"],
    },
    Case {
        name: "opencode-go-gpt-acp",
        prose: Some("OpenCode Go GPT is checking the sticky session pool next.\n"),
        prose_item_id: "opencode-go-gpt:message",
        reasoning: Some("Confirm opencode-go/gpt-5.6-luna routes through configured-acp.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "opencode-gpt-read",
            name: "Read",
            title: "Read session_scope.rs",
            arg_key: "path",
            arg_value: "src/agent_backend/session_scope.rs",
        }),
        expect_visible: &[
            "OpenCode Go GPT is checking",
            "Confirm opencode-go/gpt-5.6-luna",
            "▶ Read",
        ],
    },
    Case {
        name: "muse-acp",
        prose: Some("Muse Spark is drafting the migration notes next.\n"),
        prose_item_id: "muse:message",
        reasoning: Some("Outline the Muse Spark migration steps first.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: None,
        expect_visible: &["Muse Spark is drafting", "Outline the Muse Spark"],
    },
    Case {
        name: "spark-codex",
        prose: Some("Seasoning per-race scope safety and cache seed behavior.\n"),
        prose_item_id: "spark:message",
        reasoning: Some("Trace filter_races_by_scope before editing.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: None,
        expect_visible: &["Seasoning per-race", "Trace filter_races_by_scope"],
    },
    Case {
        name: "glm-codex",
        prose: Some("GLM is checking the Neon pooler GUCs next.\n"),
        prose_item_id: "glm:message",
        reasoning: Some("Inspect glm pooler GUCs before changing idle timeout.\n"),
        // Codex-family SubAgents often emit raw CoT as textDelta, not summary.
        reasoning_kind: ReasoningKind::RawTextDelta,
        tool: Some(Tool {
            call_id: "glm-read",
            name: "Read",
            title: "Read pooler.toml",
            arg_key: "path",
            arg_value: "pooler.toml",
        }),
        expect_visible: &["GLM is checking the Neon", "Inspect glm pooler", "▶ Read"],
    },
    Case {
        name: "fugu-codex",
        prose: None,
        prose_item_id: "fugu:message",
        reasoning: Some("Fugu should map the race filter before seeding cache.\n"),
        reasoning_kind: ReasoningKind::RawTextDelta,
        tool: Some(Tool {
            call_id: "fugu-bash",
            name: "Bash",
            title: "rg filter_races_by_scope",
            arg_key: "command",
            arg_value: "rg filter_races_by_scope",
        }),
        expect_visible: &["Fugu should map the race filter", "▶ rg"],
    },
    Case {
        name: "command-code-acp",
        prose: Some("AVITA Inc. is an avatar company founded by Hiroshi Ishiguro.\n"),
        prose_item_id: "command-code:message",
        reasoning: Some("Check AVITA Inc. official site and filings.\n"),
        reasoning_kind: ReasoningKind::Summary,
        tool: Some(Tool {
            call_id: "cmd-search",
            name: "web_search",
            title: "web_search",
            arg_key: "query",
            arg_value: "AVITA株式会社",
        }),
        expect_visible: &["AVITA Inc. is an avatar", "Check AVITA Inc. official", "▶"],
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
        .with_command_code_progress(command_code)
        .with_primed_thinking();
    feed_case_events(&mut builder, case, &sender).await;
    let mut sse = String::new();
    while let Ok(frame) = receiver.try_recv() {
        sse.push_str(&String::from_utf8_lossy(
            &frame.expect("infallible SSE frame"),
        ));
    }
    let mut live = SubAgentLiveView::default();
    live.ingest_sse(&sse);
    assert!(
        sse.contains("content_block_delta"),
        "{}: real provider output must stream on the wire: {sse}",
        case.name
    );
    assert!(
        !sse.contains('\u{200b}') && !sse.contains("claudex_activity_keepalive"),
        "{}: silent prime/keepalive must not add synthetic wire chrome: {sse}",
        case.name
    );
    if case.name == "grok-pi" {
        assert!(
            sse.contains("\"type\":\"tool_use\"") && sse.contains("input_json_delta"),
            "{}: Pi/Grok must stream native tool_use SSE before toolcall_end: {sse}",
            case.name
        );
        assert!(
            !sse.contains("▶ Read") && !sse.contains("item/providerTool"),
            "{}: Pi/Grok must not dual-paint ACP ▶ chrome: {sse}",
            case.name
        );
    }
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
        let event = match case.reasoning_kind {
            ReasoningKind::Summary => summary_reasoning_delta(case.name, reasoning),
            ReasoningKind::RawTextDelta => raw_reasoning_delta(case.name, reasoning),
        };
        builder
            .model_output_event(&event, Some(sender))
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
        if case.name == "grok-pi" {
            feed_native_tool(builder, tool, sender).await;
        } else {
            builder
                .provider_tool_call(&provider_tool(tool), Some(sender))
                .await
                .unwrap_or_else(|error| panic!("{} tool: {error}", case.name));
        }
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
                    .any(|name| name.contains(needle))
                || live
                    .visible_tool_use
                    .iter()
                    .any(|name| name.contains(needle)),
            "{}: missing `{needle}` in live viewer: thinking={:?} text={:?} server_tools={:?} tool_use={:?}",
            case.name,
            live.visible_thinking,
            live.hidden_text,
            live.visible_server_tools,
            live.visible_tool_use
        );
    }
    if case.name == "grok-pi" {
        assert_native_tool_progress(case, live);
    } else if case.tool.is_some() {
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

fn assert_native_tool_progress(case: &Case, live: &SubAgentLiveView) {
    assert_eq!(
        live.visible_tool_use,
        vec!["Read".to_owned()],
        "{}: Pi/Grok must paint a native tool_use card before toolcall_end: {:?}",
        case.name,
        live.visible_tool_use
    );
    assert!(
        live.visible_server_tools.is_empty(),
        "{}: Pi/Grok must not paint server_tool_use: {:?}",
        case.name,
        live.visible_server_tools
    );
    assert!(
        !live.visible_thinking.contains('▶'),
        "{}: native tool_use must not dual-paint ▶ thinking for the same Read: {:?}",
        case.name,
        live.visible_thinking
    );
}

async fn assert_finished_transcript(case: &Case, builder: &mut SegmentBuilder) {
    if case.prose.is_none() && case.reasoning.is_none() {
        return;
    }
    let segment = builder.finish(None).await.expect(case.name);
    if let Some(prose) = case.prose {
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
    }
    if let Some(reasoning) = case.reasoning {
        assert!(
            segment.blocks.iter().all(|block| {
                block.get("type").and_then(Value::as_str) != Some("thinking")
                    && !block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains(reasoning.trim()))
            }),
            "{}: adapter-local CoT must not be committed for replay: {:?}",
            case.name,
            segment.blocks
        );
    }
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

fn summary_reasoning_delta(case: &str, delta: &str) -> Value {
    json!({
        "method":"item/reasoning/summaryTextDelta",
        "params":{"itemId":format!("{case}:reasoning"),"summaryIndex":0,"delta":delta}
    })
}

fn raw_reasoning_delta(case: &str, delta: &str) -> Value {
    json!({
        "method":"item/reasoning/textDelta",
        "params":{"itemId":format!("{case}:reasoning"),"contentIndex":0,"delta":delta}
    })
}

async fn feed_native_tool(
    builder: &mut SegmentBuilder,
    tool: &Tool,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) {
    builder
        .start_executable_tool_use_card(tool.call_id, tool.name, Some(sender))
        .await
        .unwrap_or_else(|error| panic!("grok-pi tool start: {error}"));
    let delta = format!("{{\"{}\":\"{}\"}}", tool.arg_key, tool.arg_value);
    builder
        .append_native_tool_use_delta(tool.call_id, &delta, Some(sender))
        .await
        .unwrap_or_else(|error| panic!("grok-pi tool delta: {error}"));
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
