//! Normalize Claude Code's temporary pasted-text attachment marker.
//!
//! Claude Code represents a large paste as a short instruction pointing at a
//! file under `.codex/attachments`. Passing that marker through to a provider
//! makes it appear as an unnecessary user message. Inline the file contents at
//! the adapter boundary instead, while preserving unknown or unavailable
//! markers so no user input is silently discarded.

use std::{fs, path::Path};

use serde_json::Value;

use super::MessagesRequest;

const PREFIX: &str = "pasted text file: ";
const SUFFIX: &str = ". Read this file before continuing.";
const ATTACHMENT_DIRS: [&str; 2] = ["/.codex/attachments/", "/.claude/attachments/"];
const MAX_INLINE_BYTES: u64 = 4 * 1024 * 1024;

pub(super) fn expand_markers(request: &mut MessagesRequest) {
    for message in &mut request.messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        expand_content(content);
    }
}

fn expand_content(content: &mut Value) {
    match content {
        Value::String(text) => replace_marker(text),
        Value::Array(blocks) => blocks.iter_mut().for_each(replace_text_block),
        _ => {}
    }
}

fn replace_text_block(block: &mut Value) {
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    let Some(Value::String(text)) = block.get_mut("text") else {
        return;
    };
    replace_marker(text);
}

fn replace_marker(text: &mut String) {
    let Some(contents) = marker_contents(text) else {
        return;
    };
    *text = contents;
}

fn marker_contents(text: &str) -> Option<String> {
    let marker = text.trim();
    let path = marker.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.trim();
    if !is_attachment_path(path) {
        return None;
    }
    let path = Path::new(path);
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_INLINE_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    tracing::debug!(path = %path.display(), bytes = contents.len(), "inlined a pasted text attachment");
    Some(contents)
}

fn is_attachment_path(path: &str) -> bool {
    Path::new(path).is_absolute()
        && ATTACHMENT_DIRS
            .iter()
            .any(|directory| path.contains(directory))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "pasted_text_tests.rs"]
mod tests;
