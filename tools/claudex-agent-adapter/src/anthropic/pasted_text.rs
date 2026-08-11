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
mod tests {
    use serde_json::{Value, json};

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
    fn inlines_existing_codex_attachment_without_the_marker() {
        let root = tempfile::tempdir().expect("attachment fixture");
        let directory = root.path().join(".codex/attachments");
        std::fs::create_dir_all(&directory).expect("attachment directory");
        let path = directory.join("pasted-text-1.txt");
        std::fs::write(&path, "the pasted user instruction").expect("attachment contents");
        let marker = format!(
            "pasted text file: {}. Read this file before continuing.",
            path.display()
        );
        let mut request = request(marker.into());

        expand_markers(&mut request);

        assert_eq!(
            request.messages[0]["content"],
            "the pasted user instruction"
        );
        assert!(
            !request.messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("pasted text file:")
        );
    }

    #[test]
    fn inlines_text_blocks_but_leaves_non_text_blocks_alone() {
        let root = tempfile::tempdir().expect("attachment fixture");
        let directory = root.path().join(".claude/attachments");
        std::fs::create_dir_all(&directory).expect("attachment directory");
        let path = directory.join("pasted.txt");
        std::fs::write(&path, "block contents").expect("attachment contents");
        let marker = format!(
            "pasted text file: {}. Read this file before continuing.",
            path.display()
        );
        let mut request = request(json!([
            {"type":"text", "text":marker},
            {"type":"image", "source":{"type":"base64", "data":"abc"}}
        ]));

        expand_markers(&mut request);

        assert_eq!(request.messages[0]["content"][0]["text"], "block contents");
        assert_eq!(request.messages[0]["content"][1]["type"], "image");
    }

    #[test]
    fn preserves_prose_missing_files_and_non_user_messages() {
        let prose = "Please discuss the phrase pasted text file: literally.";
        let missing =
            "pasted text file: /.codex/attachments/missing.txt. Read this file before continuing.";
        let mut request = request(json!(prose));
        request.messages.push(json!({
            "role":"assistant",
            "content":missing
        }));
        request.messages.push(json!({
            "role":"user",
            "content":missing
        }));

        expand_markers(&mut request);

        assert_eq!(request.messages[0]["content"], prose);
        assert_eq!(request.messages[1]["content"], missing);
        assert_eq!(request.messages[2]["content"], missing);
    }

    #[test]
    fn skips_user_messages_without_content_and_non_string_text_blocks() {
        let mut request = request(json!([{"type":"text", "text":null}]));
        request.messages.insert(0, json!({"role":"user"}));
        expand_markers(&mut request);
        assert!(request.messages[0].get("content").is_none());
        assert_eq!(request.messages[1]["content"][0]["text"], Value::Null);
    }

    #[test]
    fn leaves_relative_and_non_attachment_absolute_markers_alone() {
        let relative =
            "pasted text file: .codex/attachments/local.txt. Read this file before continuing.";
        let other_absolute =
            "pasted text file: /tmp/not-an-attachment.txt. Read this file before continuing.";
        let mut request = request(json!(relative));
        request
            .messages
            .push(json!({"role":"user", "content":other_absolute}));
        expand_markers(&mut request);
        assert_eq!(request.messages[0]["content"], relative);
        assert_eq!(request.messages[1]["content"], other_absolute);
    }

    #[test]
    fn leaves_directory_and_oversized_attachment_markers_alone() {
        let root = tempfile::tempdir().expect("attachment fixture");
        let directory = root.path().join(".codex/attachments");
        std::fs::create_dir_all(&directory).expect("attachment directory");
        let dir_target = directory.join("pasted-dir");
        std::fs::create_dir(&dir_target).expect("directory attachment");
        let dir_marker = format!(
            "pasted text file: {}. Read this file before continuing.",
            dir_target.display()
        );
        let huge = directory.join("huge.txt");
        std::fs::write(&huge, vec![b'x'; (MAX_INLINE_BYTES as usize) + 1]).expect("huge file");
        let huge_marker = format!(
            "pasted text file: {}. Read this file before continuing.",
            huge.display()
        );
        let mut request = request(dir_marker.as_str().into());
        request
            .messages
            .push(json!({"role":"user", "content":huge_marker.as_str()}));
        expand_markers(&mut request);
        assert_eq!(request.messages[0]["content"], dir_marker);
        assert_eq!(request.messages[1]["content"], huge_marker);
        assert!(
            marker_contents(&dir_marker).is_none(),
            "directory attachments must not inline"
        );
        assert!(
            marker_contents(&huge_marker).is_none(),
            "oversized attachments must not inline"
        );
    }
}
