#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::Arc,
        time::Instant,
    };

    use serde_json::{Value, json};
    use tokio::sync::{Mutex, Semaphore};

    use super::{
        MAX_CONSUMED_TOOL_IDS, ToolResult, attach_mid_turn_steering, collect_tool_results,
        content_text, matching_transcript_len, mid_turn_user_steering, remember_consumed_tool_id,
        take_pending_results,
    };
    use crate::anthropic::content_batch::{batch_progress, store_batch_result};
    use crate::anthropic::{Session, agent_batch::pending_marker};

    #[tokio::test]
    async fn accepts_pending_and_already_consumed_results() {
        let active = session(
            [("pending".to_owned(), json!("call"))].into(),
            ["consumed".to_owned()].into(),
            Vec::new(),
        )
        .await;
        let results = vec![result("pending"), result("consumed")];
        let (responses, completed_tool_use_ids) = take_pending_results(&active, results)
            .await
            .expect("valid results");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].0, "call");
        assert_eq!(completed_tool_use_ids, vec!["pending"]);
        assert!(active.pending_since.lock().expect("clock").is_none());
    }

    #[tokio::test]
    async fn combines_a_complete_parallel_agent_batch_into_one_provider_result() {
        assert_complete_batch().await;
        assert_partial_batch_replays().await;
        assert_independent_batches_are_combined().await;
    }

    #[tokio::test]
    async fn accepts_repeated_batch_members_and_preserves_earlier_error_status() {
        let active = session(
            [
                ("one".to_owned(), pending_marker(json!(66), 0, 3)),
                ("two".to_owned(), pending_marker(json!(66), 1, 3)),
                ("three".to_owned(), pending_marker(json!(66), 2, 3)),
            ]
            .into(),
            HashSet::new(),
            Vec::new(),
        )
        .await;

        let (responses, completed) = take_pending_results(
            &active,
            vec![
                result("one"),
                error_result("one"),
                result("two"),
                result("three"),
            ],
        )
        .await
        .expect("duplicate batch members are retained until the batch completes");

        assert_eq!(responses.len(), 1);
        assert!(responses[0].1.is_error);
        assert_eq!(completed.len(), 3);
        assert!(active.pending_tools.lock().await.is_empty());
    }

    #[test]
    fn reports_partial_and_complete_batch_progress_without_consuming_pending() {
        let mut invalid = json!("not an object");
        store_batch_result(&mut invalid, "ignored".to_owned(), Vec::new(), false);
        assert_eq!(invalid, json!("not an object"));

        let mut pending = HashMap::from([
            ("plain".to_owned(), json!("ordinary tool")),
            ("one".to_owned(), pending_marker(json!(88), 0, 2)),
            ("two".to_owned(), pending_marker(json!(88), 1, 2)),
            ("other".to_owned(), pending_marker(json!(99), 0, 1)),
        ]);
        assert_eq!(batch_progress(&pending, &json!(88)), Some((0, 2)));
        assert_eq!(batch_progress(&pending, &json!(404)), None);

        store_batch_result(
            pending.get_mut("one").expect("first batch marker"),
            "one".to_owned(),
            vec![json!({"type":"inputText","text":"first"})],
            false,
        );
        assert_eq!(batch_progress(&pending, &json!(88)), Some((1, 2)));

        store_batch_result(
            pending.get_mut("two").expect("second batch marker"),
            "two".to_owned(),
            vec![json!({"type":"inputText","text":"second"})],
            false,
        );
        assert_eq!(batch_progress(&pending, &json!(88)), Some((2, 2)));
    }

    async fn assert_complete_batch() {
        let active = session(
            [
                ("one".to_owned(), pending_marker(json!(77), 0, 2)),
                ("two".to_owned(), pending_marker(json!(77), 1, 2)),
            ]
            .into(),
            HashSet::new(),
            Vec::new(),
        )
        .await;
        let (responses, completed_tool_use_ids) =
            take_pending_results(&active, vec![error_result("two"), result("one")])
                .await
                .expect("complete batch");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].0, 77);
        assert_eq!(completed_tool_use_ids.len(), 2);
        assert!(completed_tool_use_ids.contains(&"one".to_string()));
        assert!(completed_tool_use_ids.contains(&"two".to_string()));
        assert_eq!(
            responses[0].1.content_items[0]["text"],
            "SubAgent 1 result:"
        );
        assert_eq!(
            responses[0].1.content_items[1]["text"],
            "SubAgent 2 result:"
        );
        assert!(responses[0].1.is_error);
    }

    async fn assert_partial_batch_replays() {
        let partial = session(
            [
                ("one".to_owned(), pending_marker(json!(88), 0, 2)),
                ("two".to_owned(), pending_marker(json!(88), 1, 2)),
            ]
            .into(),
            HashSet::new(),
            Vec::new(),
        )
        .await;
        assert_first_partial_result(&partial).await;
        assert_replayed_batch_completes(&partial).await;
    }

    async fn assert_first_partial_result(partial: &Session) {
        let (responses, completed_tool_use_ids) =
            take_pending_results(partial, vec![result("one")])
                .await
                .expect("partial batch is accepted");
        assert!(responses.is_empty());
        assert!(completed_tool_use_ids.is_empty());
        let pending = partial.pending_tools.lock().await;
        assert!(pending.contains_key("one"));
        assert!(pending.contains_key("two"));
        assert_eq!(
            super::super::content_batch::batch_progress(&pending, &json!(88)),
            Some((1, 2))
        );
    }

    async fn assert_replayed_batch_completes(partial: &Session) {
        let (responses, completed_tool_use_ids) =
            take_pending_results(partial, vec![result("one")])
                .await
                .expect("partial duplicate replay is accepted");
        assert!(responses.is_empty());
        assert!(completed_tool_use_ids.is_empty());
        let (responses, _completed_tool_use_ids) =
            take_pending_results(partial, vec![result("two")])
                .await
                .expect("remaining batch result is accepted");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].0, 88);
        assert_eq!(
            responses[0].1.content_items[0]["text"],
            "SubAgent 1 result:"
        );
        assert_eq!(
            responses[0].1.content_items[1]["text"],
            "SubAgent 2 result:"
        );
        assert!(partial.pending_tools.lock().await.is_empty());
    }

    async fn assert_independent_batches_are_combined() {
        let mixed = session(
            [
                ("one".to_owned(), pending_marker(json!(77), 0, 2)),
                ("two".to_owned(), pending_marker(json!(77), 1, 2)),
                ("other-one".to_owned(), pending_marker(json!(88), 0, 2)),
                ("other-two".to_owned(), pending_marker(json!(88), 1, 2)),
            ]
            .into(),
            HashSet::new(),
            Vec::new(),
        )
        .await;
        let (responses, _completed_tool_use_ids) = take_pending_results(
            &mixed,
            vec![
                result("one"),
                result("two"),
                result("other-one"),
                result("other-two"),
            ],
        )
        .await
        .expect("complete independent batches");
        assert_eq!(responses.len(), 2);
    }

    #[tokio::test]
    async fn rejects_duplicate_unknown_and_mismatched_transcripts() {
        let active = session(
            [
                ("one".to_owned(), json!("first")),
                ("two".to_owned(), json!("second")),
            ]
            .into(),
            HashSet::new(),
            vec![json!({"role":"user","content":"original"})],
        )
        .await;
        assert!(
            take_pending_results(&active, vec![result("one"), result("one")])
                .await
                .is_err()
        );
        assert!(
            take_pending_results(&active, vec![result("unknown")])
                .await
                .is_err()
        );
        let (responses, completed_tool_use_ids) =
            take_pending_results(&active, vec![result("one")])
                .await
                .expect("one pending result");
        assert_eq!(responses.len(), 1);
        assert_eq!(completed_tool_use_ids, vec!["one"]);
        assert!(active.pending_since.lock().expect("clock").is_some());
        assert!(
            matching_transcript_len(&active, &[json!({"role":"user","content":"different"})])
                .await
                .is_none()
        );
        assert!(matching_transcript_len(&active, &[]).await.is_none());

        assert_cached_and_empty_assistant_transcripts_match().await;
    }

    async fn assert_cached_and_empty_assistant_transcripts_match() {
        let cached = session(
            HashMap::new(),
            HashSet::new(),
            vec![json!({
                "role":"user",
                "content":[{"type":"text","text":"same","cache_control":{"type":"ephemeral"}}]
            })],
        )
        .await;
        assert_eq!(
            matching_transcript_len(
                &cached,
                &[json!({"role":"user","content":[{"type":"text","text":"same"}]})]
            )
            .await,
            Some(1)
        );
        assert!(
            matching_transcript_len(
                &cached,
                &[json!({"role":"user","content":[{"type":"text","text":"changed"}]})]
            )
            .await
            .is_none()
        );

        let empty_assistant = session(
            HashMap::new(),
            HashSet::new(),
            vec![
                json!({"role":"user","content":"inspect config"}),
                json!({"role":"assistant","content":[]}),
            ],
        )
        .await;
        assert_eq!(
            matching_transcript_len(
                &empty_assistant,
                &[
                    json!({"role":"user","content":"inspect config"}),
                    json!({"role":"assistant","content":[]}),
                ],
            )
            .await,
            Some(2)
        );
        assert_eq!(
            matching_transcript_len(
                &empty_assistant,
                &[json!({"role":"user","content":"inspect config"})],
            )
            .await,
            None
        );
    }

    #[test]
    fn bounds_consumed_tool_result_replay_ids() {
        let mut consumed = HashSet::new();
        for index in 0..=MAX_CONSUMED_TOOL_IDS {
            remember_consumed_tool_id(&mut consumed, format!("tool-{index}"));
        }

        assert_eq!(consumed.len(), MAX_CONSUMED_TOOL_IDS);
        assert!(consumed.contains(&format!("tool-{MAX_CONSUMED_TOOL_IDS}")));

        remember_consumed_tool_id(&mut consumed, format!("tool-{MAX_CONSUMED_TOOL_IDS}"));
        assert_eq!(consumed.len(), MAX_CONSUMED_TOOL_IDS);
    }

    #[test]
    fn covers_text_and_transcript_short_circuit_inputs() {
        assert_eq!(content_text(&json!(null)), "");
        assert_eq!(
            content_text(&json!([
                {"type":"image","text":"ignored"},
                {"type":"text"},
                {"type":"text","text":"kept"}
            ])),
            "kept"
        );
        assert!(!super::canonical_eq(&json!([1]), &json!([])));
        assert!(!super::canonical_eq(
            &json!({"left":1}),
            &json!({"right":1})
        ));
        assert!(!super::canonical_eq(
            &json!({"left":1}),
            &json!({"left":1,"extra":2})
        ));
        assert!(super::canonical_eq(
            &json!({"cache_control":{"type":"ephemeral"}}),
            &json!({})
        ));
    }

    #[test]
    fn extracts_and_attaches_mid_turn_user_steering_beside_tool_results() {
        let message = json!({
            "role":"user",
            "content":[
                {"type":"tool_result","tool_use_id":"tool-1","content":"done"},
                {
                    "type":"text",
                    "text":"The user sent a new message while you were working:\n追加調査して\n\nAddress the message above as you continue this turn."
                }
            ]
        });
        let steering =
            mid_turn_user_steering(std::slice::from_ref(&message)).expect("mid-turn steering");
        assert!(steering.contains("追加調査して"));
        assert!(
            mid_turn_user_steering(&[json!({
                "role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tool-1","content":"done"}]
            })])
            .is_none()
        );
        assert!(
            mid_turn_user_steering(&[json!({
                "role":"user",
                "content":[{"type":"text","text":"plain follow-up"}]
            })])
            .is_none()
        );

        let mut results = vec![result("tool-1")];
        attach_mid_turn_steering(&mut results, &steering);
        assert_eq!(
            results[0]
                .content_items
                .last()
                .and_then(|item| item.get("text")),
            Some(&json!(steering))
        );

        let mut empty = Vec::new();
        attach_mid_turn_steering(&mut empty, &steering);
        assert!(
            empty.is_empty(),
            "empty tool_results must stay empty when mid-turn steering has nowhere to attach"
        );
    }

    #[test]
    fn multi_tool_result_steering_attaches_to_the_last_result_only() {
        let message = json!({
            "role":"user",
            "content":[
                {"type":"tool_result","tool_use_id":"tool-a","content":"a"},
                {"type":"tool_result","tool_use_id":"tool-b","content":"b"},
                {"type":"text","text":"The user sent a new message while you were working:\n先に tool-b の続きを優先\n\nAddress the message above as you continue this turn."}
            ]
        });
        let steering = mid_turn_user_steering(&[message]).expect("mid-turn steering");
        let mut results = vec![result("tool-a"), result("tool-b")];
        attach_mid_turn_steering(&mut results, &steering);
        assert_eq!(
            results[0].content_items.len(),
            0,
            "earlier tools stay clean"
        );
        assert_eq!(
            results[1]
                .content_items
                .last()
                .and_then(|item| item.get("text")),
            Some(&json!(steering)),
            "steering lands on the last tool_result so provider sees it after prior outputs"
        );
    }

    #[test]
    fn folds_steering_from_trailing_text_only_user_message_after_tool_results() {
        let messages = [
            json!({"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{}}]}),
            json!({
                "role":"user",
                "content":[{"type":"tool_result","tool_use_id":"tool-1","content":"done"}]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"text",
                    "text":"The user sent a new message while you were working:\n追加調査して\n\nAddress the message above as you continue this turn."
                }]
            }),
        ];
        let results = super::collect_turn_tool_results(&messages);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "tool-1");
        let steering = mid_turn_user_steering(&messages).expect("split-message steering");
        assert!(steering.contains("追加調査して"));
        let mut attached = results;
        attach_mid_turn_steering(&mut attached, &steering);
        assert!(
            attached[0].content_items.iter().any(|item| item
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| { text.contains("追加調査して") })),
            "steering from the trailing text-only user message must fold onto the tool result"
        );
    }

    #[test]
    fn mid_turn_steering_ignores_system_reminder_and_routing_noise() {
        let messages = [json!({
            "role":"user",
            "content":[
                {"type":"tool_result","tool_use_id":"tool-1","content":"done"},
                {
                    "type":"text",
                    "text":"<system-reminder>\nClaudex routing for this turn: {}\n</system-reminder>"
                },
                {
                    "type":"text",
                    "text":"The user sent a new message while you were working:\n結果を待ってから続きを\n\nAddress the message above as you continue this turn."
                }
            ]
        })];
        let steering = mid_turn_user_steering(&messages).expect("user interrupt");
        assert_eq!(steering, "結果を待ってから続きを");
        assert!(
            !steering.contains("system-reminder") && !steering.contains("Claudex routing"),
            "hook/routing chrome must not become provider steering: {steering}"
        );
        assert!(
            mid_turn_user_steering(&[json!({
                "role":"user",
                "content":[
                    {"type":"tool_result","tool_use_id":"tool-1","content":"done"},
                    {"type":"text","text":"<system-reminder>\nPostToolUse\n</system-reminder>"}
                ]
            })])
            .is_none(),
            "reminder-only trailing text must not invent steering"
        );
        assert!(
            mid_turn_user_steering(&[json!({
                "role":"user",
                "content":[
                    {"type":"tool_result","tool_use_id":"tool-1","content":"done"},
                    {"type":"text","text":"<agent-message>lifecycle only</agent-message>"},
                    {"type":"text","text":"Another Claude session sent a message"},
                    {
                        "type":"text",
                        "text":"The user sent a new message while you were working:\n実作業を続けて\n\nAddress the message above as you continue this turn."
                    }
                ]
            })])
            .as_deref()
            == Some("実作業を続けて"),
            "agent/session chrome must be filtered while real steering remains"
        );
    }

    #[test]
    fn truncates_huge_tool_result_text_before_it_is_re_sent() {
        let results = collect_tool_results(&[json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"huge",
                "content": "x".repeat(40000)
            }]
        })]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content_items[0]["text"].as_str().expect("text").len(), 32768);
        assert_eq!(
            &results[0].content_items[0]["text"].as_str().expect("text")[32724..],
            "\n[tool_result truncated to fit token budget]"
        );
    }

    fn result(tool_use_id: &str) -> ToolResult {
        ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            content_items: Vec::new(),
            is_error: false,
        }
    }

    fn error_result(tool_use_id: &str) -> ToolResult {
        ToolResult {
            is_error: true,
            ..result(tool_use_id)
        }
    }

    async fn session(
        pending_tools: HashMap<String, Value>,
        consumed_tool_ids: HashSet<String>,
        transcript: Vec<Value>,
    ) -> Session {
        let semaphore = Arc::new(Semaphore::new(1));
        Session {
            thread_id: "thread".to_owned(),
            model: "main-model".to_owned(),
            disabled_subagent_models: Default::default(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(transcript),
            pending_tools: Mutex::new(pending_tools),
            consumed_tool_ids: Mutex::new(consumed_tool_ids),
            external_tool_names: HashMap::new(),
            launch_availability: Default::default(),
            client_user_id: None,
            claude_session_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(Some(Instant::now())),
            turn_progress: Default::default(),
            adopted_thread_id: Default::default(),
            _slot: semaphore.acquire_owned().await.expect("session slot"),
        }
    }
}
