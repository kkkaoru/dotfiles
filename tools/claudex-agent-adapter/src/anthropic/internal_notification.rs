//! Fast-path handling for Claude Code's internal agent notifications.
//!
//! Native background Agent completion messages can arrive as a user-shaped
//! message. They are lifecycle signals, not new user instructions, so feeding
//! them through provider routing would make the next interactive prompt wait
//! behind an unnecessary turn.

use serde_json::Value;

use super::MessagesRequest;

mod ack;

pub(super) use ack::{acknowledge, acknowledge_with_text};

/// Remove pure Claude Code lifecycle notifications from the transcript before
/// it is reconstructed for a provider turn. Claude Code delivers these as
/// user-shaped messages, but they are not user instructions. Keeping them in
/// the transcript both inflates context and makes a delayed notification look
/// like a new user turn to a routed provider.
pub(super) fn remove_from_transcript(request: &mut MessagesRequest) {
    let messages = std::mem::take(&mut request.messages);
    request.messages = messages
        .into_iter()
        .filter_map(|mut message| {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                return Some(message);
            }
            let Some(content) = message.get("content").cloned() else {
                return Some(message);
            };
            if is_internal_notification_content(&content) {
                return None;
            }
            let Value::Array(blocks) = content else {
                return Some(message);
            };
            let block_count = blocks.len();
            let filtered = blocks
                .into_iter()
                .filter(|block| !is_internal_text_block(block))
                .collect::<Vec<_>>();
            if filtered.is_empty() {
                None
            } else if filtered.len() != block_count {
                message["content"] = Value::Array(filtered);
                Some(message)
            } else {
                Some(message)
            }
        })
        .collect();
}

pub(super) fn is_internal_notification_request(request: &MessagesRequest) -> bool {
    request
        .messages
        // Claude Code can append assistant/tool transcript elements after the
        // lifecycle user message when resuming a session. The last array
        // element is therefore not guaranteed to be the last user turn.
        .iter()
        .rev()
        // Empty user elements are transport/history separators, not a newer
        // instruction. Claude Code emits one of these around a queued
        // task-notification when a resumed session is refreshed.
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .find(|content| !is_empty_user_content(content))
        .is_some_and(is_internal_notification_content)
}

fn is_empty_user_content(content: &Value) -> bool {
    match content {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(blocks) => {
            blocks.is_empty()
                || blocks.iter().all(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind == "text")
                        && block
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| text.trim().is_empty())
                })
        }
        _ => false,
    }
}

fn is_internal_notification_content(content: &Value) -> bool {
    match content {
        Value::String(text) => is_internal_notification_text(text),
        Value::Array(blocks) if !blocks.is_empty() => blocks
            .iter()
            .try_fold(false, |found_notification, block| {
                let text = text_block(block)?;
                if text.trim().is_empty() || is_monitor_hint_text(text) {
                    return Some(found_notification);
                }
                if is_internal_notification_text(text) {
                    return Some(true);
                }
                // A real user instruction after a lifecycle block must remain
                // a normal turn rather than being swallowed as a notification.
                None
            })
            .is_some_and(|found_notification| found_notification),
        _ => false,
    }
}

fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

fn is_internal_text_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(is_internal_notification_text)
}

fn is_internal_notification_text(text: &str) -> bool {
    let text = text.trim();
    [
        ("<agent-message", "</agent-message>"),
        ("<task-notification", "</task-notification>"),
    ]
    .into_iter()
    .any(|(opening, closing)| {
        let Some(end) = text.find(closing) else {
            return false;
        };
        text.starts_with(opening)
            && (text[end + closing.len()..].trim().is_empty()
                || is_monitor_hint_text(&text[end + closing.len()..]))
    }) || text.starts_with("Another Claude session sent a message")
}

fn is_monitor_hint_text(text: &str) -> bool {
    text.trim_start()
        .starts_with("If this event is something the user would act on now")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(content: Value) -> MessagesRequest {
        MessagesRequest {
            model: "test-model".to_owned(),
            system: Value::Null,
            messages: vec![json!({"role":"user", "content":content})],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    #[test]
    fn recognizes_only_pure_internal_agent_notifications() {
        assert!(is_internal_notification_request(&request(
            "<agent-message from=\"general-purpose\">worker output</agent-message>".into()
        )));
        assert!(is_internal_notification_request(&request(
            json!([{"type":"text","text":"<task-notification>done</task-notification>"}])
        )));
        assert!(!is_internal_notification_request(&request(
            "Please inspect the literal <agent-message> tag".into()
        )));
        assert!(!is_internal_notification_request(&request(json!([{
            "type":"tool_result",
            "tool_use_id":"toolu_internal",
            "content":"<agent-message>result</agent-message>"
        }]))));
    }

    #[test]
    fn recognizes_notification_with_trailing_transcript_elements() {
        let mut request =
            request("<task-notification><status>completed</status></task-notification>".into());
        request.messages.push(json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "acknowledged"}]
        }));

        assert!(is_internal_notification_request(&request));
    }

    #[test]
    fn recognizes_monitor_notification_with_standard_hint_block() {
        assert!(is_internal_notification_request(&request(json!([
            {
                "type":"text",
                "text":"<task-notification><status>completed</status></task-notification>"
            },
            {
                "type":"text",
                "text":"If this event is something the user would act on now, send a PushNotification."
            }
        ]))));
    }

    #[test]
    fn keeps_real_instruction_after_notification_as_a_user_turn() {
        assert!(!is_internal_notification_request(&request(json!([
            {
                "type":"text",
                "text":"<task-notification><status>completed</status></task-notification>"
            },
            {"type":"text","text":"continue with the requested change"}
        ]))));
    }

    #[test]
    fn does_not_ack_notification_when_a_newer_user_turn_exists() {
        let mut request =
            request("<task-notification><status>completed</status></task-notification>".into());
        request.messages.push(json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "acknowledged"}]
        }));
        request
            .messages
            .push(json!({"role": "user", "content": "continue with the requested change"}));

        assert!(!is_internal_notification_request(&request));
    }

    #[test]
    fn ignores_empty_user_history_after_notification() {
        let mut request =
            request("<task-notification><status>completed</status></task-notification>".into());
        request.messages.push(json!({"role":"user", "content":""}));
        request
            .messages
            .push(json!({"role":"assistant", "content":[]}));

        assert!(is_internal_notification_request(&request));
    }

    #[test]
    fn ignores_empty_text_blocks_after_notification() {
        let mut request =
            request("<task-notification><status>completed</status></task-notification>".into());
        request.messages.push(json!({
            "role":"user",
            "content":[{"type":"text","text":""}]
        }));

        assert!(is_internal_notification_request(&request));
    }

    #[test]
    fn keeps_tool_result_after_notification_as_a_real_turn() {
        let mut request =
            request("<task-notification><status>completed</status></task-notification>".into());
        request.messages.push(json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"toolu_real",
                "content":"provider result"
            }]
        }));

        assert!(!is_internal_notification_request(&request));
    }

    #[test]
    fn removes_internal_history_and_keeps_real_user_blocks() {
        let mut request = request(json!([{
            "type":"text",
            "text":"real instruction"
        }]));
        request.messages = vec![
            json!({"role":"user","content":"first instruction"}),
            json!({"role":"user","content":"<agent-message from=\"worker\">done</agent-message>"}),
            json!({"role":"user","content":[
                {"type":"text","text":"<task-notification>done</task-notification>"},
                {"type":"text","text":"latest instruction"}
            ]}),
        ];

        remove_from_transcript(&mut request);

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0]["content"], "first instruction");
        assert_eq!(
            request.messages[1]["content"][0]["text"],
            "latest instruction"
        );
    }

    #[test]
    fn drops_empty_user_arrays_and_keeps_contentless_user_messages() {
        let mut req = request("keep me".into());
        req.messages = vec![
            json!({"role":"user"}),
            json!({"role":"user","content":[]}),
            json!({"role":"user","content":[
                {"type":"text","text":"  "},
                {"type":"text","text":"<agent-message from=\"w\">done</agent-message>"}
            ]}),
            json!({"role":"user","content":"keep me"}),
        ];
        assert!(is_internal_notification_request(&request(json!([
            {"type":"text","text":"  "},
            {"type":"text","text":"<agent-message from=\"w\">done</agent-message>"}
        ]))));
        assert!(!is_internal_notification_request(&request(json!([]))));
        remove_from_transcript(&mut req);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0], json!({"role":"user"}));
        assert_eq!(req.messages[1]["content"], "keep me");
    }

    #[test]
    fn preserves_teammate_wrappers_that_carry_a_subagent_prompt() {
        let mut request = request(
            "<teammate-message>Use the assigned model and report the result.</teammate-message>"
                .into(),
        );

        remove_from_transcript(&mut request);

        assert_eq!(request.messages.len(), 1);
        assert!(
            request.messages[0]["content"]
                .as_str()
                .is_some_and(|text| text.contains("Use the assigned model"))
        );
    }
}
