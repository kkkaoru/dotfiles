//! Fast-path classification for Claude Code lifecycle notifications.

#[cfg(test)]
use anyhow::Result;
use serde_json::Value;

const LIFECYCLE_ORIGINS: &[&str] = &[
    "agent-message",
    "agent-notification",
    "subagent-notification",
    "task-notification",
];
const CROSS_SESSION_PREFIX: &str = "Another Claude session sent a message:";

/// Return Claude's current prompt field, accepting the legacy spelling only
/// when the current field is absent.
pub fn prompt(payload: &Value) -> Option<&str> {
    match payload.get("prompt") {
        Some(value) => value.as_str(),
        None => payload.get("user_prompt").and_then(Value::as_str),
    }
}

fn origin_is_lifecycle(payload: &Value) -> bool {
    payload
        .get("origin")
        .and_then(|origin| origin.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| LIFECYCLE_ORIGINS.contains(&kind))
}

fn wrapped_lifecycle_message(text: &str) -> bool {
    LIFECYCLE_ORIGINS
        .iter()
        .any(|tag| exact_envelope(text, tag))
}

fn exact_envelope(text: &str, tag: &str) -> bool {
    let opening = format!("<{tag}");
    let Some(after_name) = text.strip_prefix(&opening) else {
        return false;
    };
    if !after_name
        .as_bytes()
        .first()
        .is_some_and(|byte| *byte == b'>' || byte.is_ascii_whitespace())
    {
        return false;
    }
    let Some(open_end) = text.find('>') else {
        return false;
    };
    let closing = format!("</{tag}>");
    let Some(close_start) = text.len().checked_sub(closing.len()) else {
        return false;
    };
    open_end < close_start
        && text.ends_with(&closing)
        && text[open_end + 1..]
            .find(&closing)
            .is_some_and(|offset| open_end + 1 + offset == close_start)
}

fn displayed_agent_completion(text: &str) -> bool {
    if text.contains('\r') || text.contains('\n') {
        return false;
    }
    let Some(rest) = text.strip_prefix("Agent \"") else {
        return false;
    };
    let Some((name, status)) = rest.rsplit_once("\" ") else {
        return false;
    };
    if name.trim().is_empty() {
        return false;
    }
    status
        .strip_prefix("finished · ")
        .is_some_and(valid_duration)
        || status
            .strip_prefix("failed: ")
            .is_some_and(|reason| !reason.trim().is_empty())
        || status == "stopped"
}

fn valid_duration(text: &str) -> bool {
    let mut found = false;
    for token in text.split_ascii_whitespace() {
        found = true;
        let digits = token
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 || !matches!(&token[digits..], "ms" | "s" | "m" | "h" | "d") {
            return false;
        }
    }
    found
}

pub fn is_lifecycle_notification(payload: &Value) -> bool {
    let prompt_is_present = payload.get("prompt").is_some() || payload.get("user_prompt").is_some();
    if !prompt_is_present {
        return origin_is_lifecycle(payload);
    }
    let Some(text) = prompt(payload).map(str::trim) else {
        return false;
    };
    if wrapped_lifecycle_message(text) || displayed_agent_completion(text) {
        return true;
    }
    let Some(remainder) = text.strip_prefix(CROSS_SESSION_PREFIX) else {
        return false;
    };
    wrapped_lifecycle_message(remainder.trim())
}

/// Lifecycle notifications receive no routing context and cannot enter slow I/O.
#[cfg(test)]
pub fn dispatch<F>(event_name: &str, payload: Option<&Value>, slow_path: F) -> Result<Value>
where
    F: FnOnce() -> Result<Value>,
{
    if event_name == "UserPromptSubmit" && payload.is_some_and(is_lifecycle_notification) {
        return Ok(serde_json::json!({}));
    }
    slow_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn completed_task_payload() -> Value {
        serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": concat!(
                "<task-notification>\n",
                "<task-id>a760b564e16f0c75b</task-id>\n",
                "<tool-use-id>toolu_01PZTL3F1cmX4HzhCH8k79dP</tool-use-id>\n",
                "<status>completed</status>\n",
                "<summary>Agent \"Run autonomous 768x432/10s smoke to SeedVR2 1080p\" finished</summary>\n",
                "</task-notification>"
            )
        })
    }

    #[test]
    fn exact_completed_task_payload_performs_zero_slow_io() {
        let config_reads = Cell::new(0);
        let cache_reads = Cell::new(0);
        let process_starts = Cell::new(0);
        let output = dispatch("UserPromptSubmit", Some(&completed_task_payload()), || {
            config_reads.set(config_reads.get() + 1);
            cache_reads.set(cache_reads.get() + 1);
            process_starts.set(process_starts.get() + 1);
            Ok(serde_json::json!({"unexpected": true}))
        })
        .unwrap();

        assert_eq!(output, serde_json::json!({}));
        assert_eq!(config_reads.get(), 0);
        assert_eq!(cache_reads.get(), 0);
        assert_eq!(process_starts.get(), 0);
    }

    #[test]
    fn recognizes_all_exact_notification_shapes() {
        for payload in [
            serde_json::json!({"prompt": "Agent \"worker\" finished · 18m 32s"}),
            serde_json::json!({"prompt": "Agent \"worker\" failed: API error"}),
            serde_json::json!({"prompt": "Agent \"worker\" stopped"}),
            serde_json::json!({
                "user_prompt": "Another Claude session sent a message: <agent-message from=\"worker\">done</agent-message>"
            }),
            serde_json::json!({"prompt": "<task-notification>done</task-notification>"}),
            serde_json::json!({"prompt": "<agent-notification>done</agent-notification>"}),
            serde_json::json!({"prompt": "<subagent-notification>done</subagent-notification>"}),
            serde_json::json!({"origin": {"kind": "agent-notification"}}),
        ] {
            assert!(is_lifecycle_notification(&payload));
        }
    }

    #[test]
    fn rejects_trailing_human_text_and_lifecycle_looking_prose() {
        for payload in [
            serde_json::json!({
                "prompt": "<task-notification>done</task-notification>\nPlease continue the real task"
            }),
            serde_json::json!({
                "prompt": concat!(
                    "Another Claude session sent a message: ",
                    "<agent-message>done</agent-message> now fix the bug"
                )
            }),
            serde_json::json!({"prompt": "Agent \"worker\" stopped responding; diagnose it"}),
            serde_json::json!({"prompt": "Agent \"worker\" finished · recently"}),
            serde_json::json!({
                "prompt": "<task-notification-archive>done</task-notification-archive>"
            }),
        ] {
            assert!(!is_lifecycle_notification(&payload));
        }
    }

    #[test]
    fn present_prompt_overrides_origin_and_legacy_fields() {
        let lifecycle = "<task-notification>done</task-notification>";
        for (payload, expected) in [
            (
                serde_json::json!({
                    "prompt": "Please continue the real task",
                    "origin": {"kind": "task-notification"}
                }),
                false,
            ),
            (
                serde_json::json!({
                    "prompt": "Please continue the real task",
                    "user_prompt": lifecycle
                }),
                false,
            ),
            (
                serde_json::json!({
                    "prompt": lifecycle,
                    "user_prompt": "Please continue the real task"
                }),
                true,
            ),
            (
                serde_json::json!({
                    "prompt": null,
                    "user_prompt": lifecycle,
                    "origin": {"kind": "task-notification"}
                }),
                false,
            ),
            (
                serde_json::json!({
                    "prompt": 42,
                    "user_prompt": lifecycle
                }),
                false,
            ),
        ] {
            assert_eq!(is_lifecycle_notification(&payload), expected, "{payload}");
        }
    }

    #[test]
    fn malformed_or_nonobject_payloads_are_not_notifications() {
        for payload in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!("<task-notification>done</task-notification>"),
            serde_json::json!({"prompt": []}),
            serde_json::json!({"user_prompt": {"text": "notification"}}),
            serde_json::json!({
                "prompt": false,
                "origin": {"kind": "task-notification"}
            }),
        ] {
            assert!(!is_lifecycle_notification(&payload), "{payload}");
        }
    }

    #[test]
    fn normal_prompt_routes_through_the_slow_path() {
        let calls = Cell::new(0);
        let payload = serde_json::json!({
            "prompt": "Explain why an Agent finished notification can appear."
        });
        let output = dispatch("UserPromptSubmit", Some(&payload), || {
            calls.set(calls.get() + 1);
            Ok(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": "routing-context"
                }
            }))
        })
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(
            output["hookSpecificOutput"]["additionalContext"],
            "routing-context"
        );
    }

    #[test]
    fn literal_tags_inside_a_user_question_are_not_lifecycle_messages() {
        let payload = serde_json::json!({
            "prompt": "Explain the literal <task-notification> tag"
        });
        assert!(!is_lifecycle_notification(&payload));
    }
}
