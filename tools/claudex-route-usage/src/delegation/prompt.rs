use super::MAX_SESSION_ID_BYTES;
#[cfg(test)]
use super::STATE_DIRECTORY;
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::path::{Path, PathBuf};

/// Prefer Claude Code's current spelling. A present but invalid current field
/// is not rescued by the legacy spelling.
pub fn session_id(payload: &Value) -> Option<&str> {
    let value = match payload.get("session_id") {
        Some(value) => value,
        None => payload.get("sessionId")?,
    };
    valid_session_id(value)
}

fn valid_session_id(value: &Value) -> Option<&str> {
    let id = value.as_str()?.trim();
    (!id.is_empty() && id.len() <= MAX_SESSION_ID_BYTES && !id.chars().any(char::is_control))
        .then_some(id)
}

pub fn session_key(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

#[cfg(test)]
pub fn state_path(cache_dir: &Path, id: &str) -> PathBuf {
    cache_dir
        .join(STATE_DIRECTORY)
        .join(format!("{}.json", session_key(id)))
}

fn normalized_prompt(prompt: &str) -> String {
    prompt
        .chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' | '\u{02bc}' | '\u{ff07}' => '\'',
            _ if character.is_ascii() => character.to_ascii_lowercase(),
            _ => character,
        })
        .collect()
}

fn word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn contains_bounded_literal(text: &str, literal: &str) -> bool {
    text.match_indices(literal).any(|(offset, matched)| {
        let before = text[..offset].chars().next_back();
        let after = text[offset + matched.len()..].chars().next();
        before.is_none_or(|character| !word_character(character))
            && after.is_none_or(|character| !word_character(character))
    })
}

pub fn current_prompt_opts_out(payload: &Value) -> bool {
    let Some(prompt) = crate::lifecycle::prompt(payload) else {
        return false;
    };
    let normalized = normalized_prompt(prompt);
    let english = [
        "do not delegate",
        "don't delegate",
        "dont delegate",
        "no delegation",
        "without delegation",
        "do not use subagents",
        "don't use subagents",
        "dont use subagents",
        "no subagents",
    ];
    english
        .into_iter()
        .any(|phrase| contains_bounded_literal(&normalized, phrase))
        || ["委譲しないで", "委譲しない", "サブエージェントを使わない"]
            .into_iter()
            .any(|phrase| normalized.contains(phrase))
}

/// Make the routing metadata reflect an explicit opt-out in this prompt. The
/// unmodified branch deliberately leaves genuine worker selections intact.
pub fn effective_summary(mut summary: Value, payload: Option<&Value>) -> Value {
    let opted_out = payload.is_some_and(current_prompt_opts_out);
    let effective_worker_count = crate::hook::effective_workers(&summary).len();
    let Some(object) = summary.as_object_mut() else {
        return summary;
    };
    let base = object
        .get("delegation_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && effective_worker_count > 0;
    let required = base && !opted_out;
    object.insert("base_delegation_required".into(), Value::Bool(base));
    object.insert("prompt_delegation_opt_out".into(), Value::Bool(opted_out));
    object.insert("delegation_required".into(), Value::Bool(required));
    object.insert(
        "direct_main_execution".into(),
        Value::from(if required { "fallback-only" } else { "allowed" }),
    );
    summary
}
