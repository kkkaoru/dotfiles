use serde_json::{Value, json};

use super::{
    MAX_TURN_INPUT_BYTES, TRUNCATED_HISTORY_HEADER, TRUNCATED_INPUT_NOTICE, full_transcript_input,
    user_input_from_messages,
};

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

fn input_text_bytes(item: &Value) -> usize {
    item["text"].as_str().map_or(0, str::len)
}
