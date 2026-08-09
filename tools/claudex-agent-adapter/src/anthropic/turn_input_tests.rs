use serde_json::{Value, json};

use super::{
    FULL_HISTORY_HEADER, MAX_TURN_INPUT_BYTES, TRUNCATED_HISTORY_HEADER, TRUNCATED_INPUT_NOTICE,
    bound_input, full_transcript_input, full_transcript_input_with_token_budget, input_bytes,
    oversized_latest_message, user_input_from_messages, utf8_suffix,
};

#[test]
fn handles_small_empty_and_mixed_message_inputs() {
    let single_user = full_transcript_input(&[json!({"role":"user","content":"question"})]);
    assert_eq!(
        single_user,
        user_input_from_messages(&[json!({
            "role":"user",
            "content":"question"
        })])
    );
    let single_assistant = full_transcript_input(&[json!({
        "role":"assistant",
        "content":"answer"
    })]);
    assert!(
        single_assistant[0]["text"]
            .as_str()
            .unwrap()
            .contains("answer")
    );
    let transcript = full_transcript_input(&[
        json!({"role":"assistant","content":"answer"}),
        json!({"role":"user","content":"question"}),
    ]);
    assert!(
        transcript[0]["text"]
            .as_str()
            .unwrap()
            .starts_with(FULL_HISTORY_HEADER)
    );
    let command_code = super::provider_turn_input(
        "meta/muse-spark-1.2-contributor",
        &[
            json!({"role":"assistant","content":"Ready to continue"}),
            json!({"role":"user","content":"Read CLAUDE.md and output only the first heading."}),
        ],
    );
    assert_eq!(
        command_code[0]["text"],
        "Read CLAUDE.md and output only the first heading."
    );
    assert!(
        !command_code[0]["text"]
            .as_str()
            .unwrap()
            .contains("role-tagged history")
    );
    assert_eq!(
        user_input_from_messages(&[]),
        vec![json!({"type":"text","text":"Continue."})]
    );
    let input = user_input_from_messages(&[
        json!({"role":"assistant","content":"ignored"}),
        json!({"role":"user","content":[
            {"type":"text","text":"visible"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}},
            {"type":"unknown"}
        ]}),
    ]);
    assert_eq!(input[0]["text"], "visible");
    assert_eq!(input[1]["url"], "data:image/png;base64,abc");
}

#[test]
fn bounds_reconstructed_history_at_complete_message_boundaries() {
    let mut messages = vec![json!({"role":"user","content":format!(
        "OLDEST_SENTINEL{}", "a".repeat(200_000)
    )})];
    for index in 0..5 {
        messages.push(json!({
            "role":if index % 2 == 0 { "assistant" } else { "user" },
            "content":format!("message-{index}-{}", "b".repeat(200_000))
        }));
    }
    messages.push(json!({"role":"user","content":"LATEST_SENTINEL"}));

    let input = full_transcript_input(&messages);
    let text = input[0]["text"].as_str().expect("history text");
    assert!(text.len() <= MAX_TURN_INPUT_BYTES);
    assert!(text.starts_with(TRUNCATED_HISTORY_HEADER));
    assert!(text.contains("LATEST_SENTINEL"));
    assert!(!text.contains("OLDEST_SENTINEL"));
    let retained = text.strip_prefix(TRUNCATED_HISTORY_HEADER).expect("header");
    let retained: Value = serde_json::from_str(retained).expect("valid retained history");
    assert_eq!(
        retained.as_array().and_then(|items| items.last()).unwrap()["content"],
        "LATEST_SENTINEL"
    );
}

#[test]
fn applies_a_provider_token_budget_to_reconstructed_history() {
    let messages = (0..20)
        .map(|index| {
            json!({
                "role": if index % 2 == 0 { "assistant" } else { "user" },
                "content": format!("message-{index}-{}", "x".repeat(500))
            })
        })
        .collect::<Vec<_>>();
    let input = full_transcript_input_with_token_budget(&messages, 1_000);
    let text = input[0]["text"].as_str().expect("history text");
    assert!(text.len() <= 4_000);
    assert!(text.starts_with(TRUNCATED_HISTORY_HEADER));
    assert!(text.contains("message-19"));
}

#[test]
fn applies_a_provider_token_budget_to_a_single_large_user_message() {
    let input = full_transcript_input_with_token_budget(
        &[json!({"role":"user", "content":"x".repeat(10_000)})],
        1_000,
    );
    let text = input.iter().map(input_text_bytes).sum::<usize>();
    assert!(text <= 4_000);
    assert_eq!(input[0]["text"], TRUNCATED_INPUT_NOTICE);
}

#[test]
fn bounds_incremental_teammate_bursts_and_keeps_latest_input() {
    let input = user_input_from_messages(&[json!({
        "role":"user",
        "content":[
            {"type":"text","text":format!("OLD_RESULT{}", "x".repeat(700_000))},
            {"type":"text","text":format!("LATEST_RESULT{}", "y".repeat(200_000))}
        ]
    })]);

    assert_eq!(input[0]["text"], TRUNCATED_INPUT_NOTICE);
    assert!(input.iter().map(input_text_bytes).sum::<usize>() <= MAX_TURN_INPUT_BYTES);
    assert!(
        input.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .starts_with("LATEST_RESULT")
    );
    assert!(
        input
            .iter()
            .all(|item| !item.to_string().contains("OLD_RESULT"))
    );
}

#[test]
fn truncates_one_oversized_latest_message_on_a_utf8_boundary() {
    let input = user_input_from_messages(&[json!({
        "role":"user", "content":format!("{}LATEST_SUFFIX", "界".repeat(400_000))
    })]);

    assert_eq!(input[0]["text"], TRUNCATED_INPUT_NOTICE);
    assert!(input.iter().map(input_text_bytes).sum::<usize>() <= MAX_TURN_INPUT_BYTES);
    assert!(
        input.last().unwrap()["text"]
            .as_str()
            .unwrap()
            .ends_with("LATEST_SUFFIX")
    );
}

#[test]
fn truncates_an_oversized_latest_history_message_and_covers_byte_helpers() {
    let input = full_transcript_input(&[
        json!({"role":"assistant","content":"old"}),
        json!({"role":"user","content":"x".repeat(MAX_TURN_INPUT_BYTES + 1)}),
    ]);
    let text = input[0]["text"].as_str().unwrap();
    assert!(text.starts_with(TRUNCATED_HISTORY_HEADER));
    assert!(text.contains("truncated_message_suffix"));
    assert_eq!(oversized_latest_message(None, 10), "[]");
    assert_eq!(utf8_suffix("short", 10), "short");
    assert_eq!(utf8_suffix("a界b", 2), "b");
    assert_eq!(input_bytes(&json!({"url":"abc"})), 3);
    assert!(input_bytes(&json!({"other":1})) > 0);
}

#[test]
fn bounds_non_text_and_exact_budget_inputs() {
    let oversized = json!({"blob":"x".repeat(MAX_TURN_INPUT_BYTES)});
    assert_eq!(bound_input(vec![oversized]).len(), 1);
    let exact_remaining = MAX_TURN_INPUT_BYTES - TRUNCATED_INPUT_NOTICE.len();
    let bounded = bound_input(vec![
        json!({"type":"text","text":"p".repeat(TRUNCATED_INPUT_NOTICE.len() + 1)}),
        json!({"type":"text","text":"x".repeat(exact_remaining)}),
    ]);
    assert_eq!(bounded[0]["text"], TRUNCATED_INPUT_NOTICE);
    assert_eq!(bounded.len(), 2);
}

fn input_text_bytes(item: &Value) -> usize {
    item["text"].as_str().map_or(0, str::len)
}
