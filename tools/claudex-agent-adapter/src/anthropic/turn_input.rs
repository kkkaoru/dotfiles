use serde_json::{Value, json};

use super::content::serialized_len;

// Codex app-server rejects turn input above 1 MiB. Leave generous headroom for
// JSON-RPC framing, model metadata, and serialized events.
pub(super) const MAX_TURN_INPUT_BYTES: usize = 512 * 1_024;
const FULL_HISTORY_HEADER: &str =
    "Continue this Claude Code conversation. The role-tagged history follows:\n";
const TRUNCATED_HISTORY_HEADER: &str = "Continue this Claude Code conversation. Earlier completed history was omitted to fit the provider input limit. The retained role-tagged history follows:\n";
pub(super) const TRUNCATED_INPUT_NOTICE: &str =
    "[Earlier turn input was omitted to fit the provider input limit.]";

pub(super) fn full_transcript_input(messages: &[Value]) -> Vec<Value> {
    full_transcript_input_with_byte_limit(messages, MAX_TURN_INPUT_BYTES)
}

/// Command Code Muse Spark is one-shot `cmd -p`. Reconstructed Claude history
/// makes it greet / ignore the delegated task. Follow-ups must keep only the
/// latest user instruction; concatenating earlier user turns caused Spark to
/// keep working the stale task.
pub(super) fn provider_turn_input(model: &str, messages: &[Value]) -> Vec<Value> {
    if crate::command_code_acp::is_command_code_model(model) {
        return latest_user_input_from_messages(messages);
    }
    full_transcript_input(messages)
}

pub(super) fn provider_turn_input_with_token_budget(
    model: &str,
    messages: &[Value],
    token_budget: usize,
) -> Vec<Value> {
    if crate::command_code_acp::is_command_code_model(model) {
        return latest_user_input_from_messages_with_byte_limit(
            messages,
            token_budget.saturating_mul(4).min(MAX_TURN_INPUT_BYTES),
        );
    }
    full_transcript_input_with_token_budget(messages, token_budget)
}

fn latest_user_input_from_messages(messages: &[Value]) -> Vec<Value> {
    latest_user_input_from_messages_with_byte_limit(messages, MAX_TURN_INPUT_BYTES)
}

fn latest_user_input_from_messages_with_byte_limit(
    messages: &[Value],
    max_bytes: usize,
) -> Vec<Value> {
    let Some(latest) = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return user_input_from_messages_with_byte_limit(&[], max_bytes);
    };
    user_input_from_messages_with_byte_limit(std::slice::from_ref(latest), max_bytes)
}

pub(super) fn full_transcript_input_with_token_budget(
    messages: &[Value],
    token_budget: usize,
) -> Vec<Value> {
    full_transcript_input_with_byte_limit(
        messages,
        token_budget.saturating_mul(4).min(MAX_TURN_INPUT_BYTES),
    )
}

fn full_transcript_input_with_byte_limit(messages: &[Value], max_bytes: usize) -> Vec<Value> {
    if messages.len() == 1 && messages[0].get("role").and_then(Value::as_str) == Some("user") {
        return user_input_from_messages_with_byte_limit(messages, max_bytes);
    }
    let (header, history) = bounded_history(messages, max_bytes);
    vec![json!({"type":"text", "text":format!("{header}{history}")})]
}

pub(super) fn user_input_from_messages(messages: &[Value]) -> Vec<Value> {
    user_input_from_messages_with_byte_limit(messages, MAX_TURN_INPUT_BYTES)
}

/// `turn/start` user payload. Command Code must never concatenate prior user
/// turns, including the new-session transcript path that used to bypass
/// [`provider_turn_input`].
pub(super) fn provider_user_turn_input(model: &str, messages: &[Value]) -> Vec<Value> {
    if crate::command_code_acp::is_command_code_model(model) {
        latest_user_input_from_messages(messages)
    } else {
        user_input_from_messages(messages)
    }
}

fn user_input_from_messages_with_byte_limit(messages: &[Value], max_bytes: usize) -> Vec<Value> {
    let mut input = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .flat_map(message_input)
        .collect::<Vec<_>>();
    if input.is_empty() {
        input.push(json!({"type":"text", "text":"Continue."}));
    }
    bound_input_with_byte_limit(input, max_bytes)
}

fn bounded_history(messages: &[Value], max_bytes: usize) -> (&'static str, String) {
    let original_bytes = serialized_len(&messages);
    if original_bytes + FULL_HISTORY_HEADER.len() <= max_bytes {
        return (
            FULL_HISTORY_HEADER,
            serde_json::to_string(messages).unwrap_or_default(),
        );
    }
    let budget = max_bytes.saturating_sub(TRUNCATED_HISTORY_HEADER.len());
    let mut start = messages.len();
    let mut retained_bytes = 2;
    for (index, message) in messages.iter().enumerate().rev() {
        let separator = usize::from(start < messages.len());
        let next = serialized_len(message) + separator;
        if retained_bytes + next > budget {
            break;
        }
        retained_bytes += next;
        start = index;
    }
    let history = if start == messages.len() {
        oversized_latest_message(messages.last(), budget)
    } else {
        serde_json::to_string(&messages[start..]).unwrap_or_default()
    };
    tracing::warn!(
        original_bytes,
        retained_messages = messages.len().saturating_sub(start),
        "truncated reconstructed transcript before provider turn/start"
    );
    (TRUNCATED_HISTORY_HEADER, history)
}

fn oversized_latest_message(message: Option<&Value>, budget: usize) -> String {
    let Some(message) = message else {
        return "[]".to_owned();
    };
    let serialized = serde_json::to_string(message).unwrap_or_default();
    let excerpt = utf8_suffix(&serialized, budget.min(MAX_TURN_INPUT_BYTES / 4));
    serde_json::to_string(&json!([{
        "role":message.get("role").and_then(Value::as_str).unwrap_or("unknown"),
        "truncated_message_suffix":excerpt
    }]))
    .unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(test)]
fn bound_input(input: Vec<Value>) -> Vec<Value> {
    bound_input_with_byte_limit(input, MAX_TURN_INPUT_BYTES)
}

#[path = "turn_input_bound.rs"]
mod bound;
use bound::{bound_input_with_byte_limit, message_input, utf8_suffix};
#[cfg(test)]
use bound::input_bytes;


#[cfg(test)]
#[path = "turn_input_tests.rs"]
mod tests;
