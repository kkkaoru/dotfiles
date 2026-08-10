use serde_json::Value;

use crate::anthropic::MessagesRequest;

const MAX_STATED_SCOPES: usize = 40;
const CARDINALITY_WINDOW: usize = 24;

pub(crate) fn has_parallel_scope(request: &MessagesRequest) -> bool {
    independent_scope_count(request) >= 2
}

pub(crate) fn independent_scope_count(request: &MessagesRequest) -> usize {
    match last_real_user_text(request) {
        Some(content) => count_for_content(&content),
        // Reconstructed transcripts keep a parallel baseline for floor/replenishment.
        None => 2,
    }
}

pub(super) fn has_classifiable_user_turn(request: &MessagesRequest) -> bool {
    last_real_user_text(request).is_some()
}

fn last_real_user_text(request: &MessagesRequest) -> Option<String> {
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(user_message_text)
        .filter(|content| !content.trim_start().starts_with("<task-notification>"))
        .rev()
        .find(|content| !is_remaining_only_follow_up(content))
}

fn user_message_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn count_for_content(content: &str) -> usize {
    if contains_single_scope_request(content) {
        return 1;
    }
    if let Some(stated) = explicit_scope_cardinality(content) {
        return stated;
    }
    let explicit_blocks = count_explicit_blocks(content);
    if explicit_blocks >= 2 {
        return explicit_blocks;
    }
    if contains_parallel_intent(content) {
        2
    } else {
        1
    }
}

pub(crate) fn needs_single_worker(request: &MessagesRequest) -> bool {
    if independent_scope_count(request) >= 2 {
        return false;
    }
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(user_message_text)
        .any(|content| is_atomic_lookup(&content))
}

fn is_atomic_lookup(content: &str) -> bool {
    if contains_single_scope_request(content) {
        return true;
    }
    let trimmed = content.trim();
    let lower = trimmed.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    matches!(first, "gh" | "git" | "curl" | "wget")
        || ((lower.starts_with("http://") || lower.starts_with("https://"))
            && !trimmed.contains('\n')
            && trimmed.split_whitespace().count() <= 3)
}

pub(super) fn is_substantive_work(request: &MessagesRequest) -> bool {
    let Some(content) = last_real_user_text(request) else {
        return false;
    };
    count_for_content(&content) >= 2
        || contains_parallel_intent(&content)
        || contains_substantive_verb(&content)
}

fn contains_substantive_verb(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    [
        "investigate",
        "implement",
        "review",
        "research",
        "refactor",
        "debug",
        "analyze",
        "調査",
        "実装",
        "修正",
        "検証",
        "設計",
        "テスト",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

fn contains_single_scope_request(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    [
        "exactly one",
        "one worker",
        "one subagent",
        "1つのsubagent",
        "1つのsub agent",
        "単一",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

fn contains_parallel_intent(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    [
        "parallel",
        "multiple",
        "independent",
        "in parallel",
        "複数",
        "並列",
        "分担",
        "各観点",
        "比較",
        "独立スコープ",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

fn explicit_scope_cardinality(content: &str) -> Option<usize> {
    let lower = content.to_ascii_lowercase();
    let keywords = [
        "independent scope",
        "independent scopes",
        "独立スコープ",
        "スコープ",
        "subagent",
        "sub agent",
        "worker",
        "workers",
        "ワーカー",
        "サブエージェント",
        "parallel worker",
        "parallel workers",
    ];
    let mut best = None;
    let mut index = 0;
    while index < lower.len() {
        let rest = &lower[index..];
        let Some(digit_offset) = rest.find(|character: char| character.is_ascii_digit()) else {
            break;
        };
        let start = index + digit_offset;
        if start > 0
            && lower[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_digit())
        {
            index = start + 1;
            continue;
        }
        if is_remaining_count_prefix(&lower[..start]) {
            index = start + 1;
            continue;
        }
        let digits: String = lower[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let parsed = digits.parse::<usize>().ok();
        index = start + digits.len().max(1);
        let Some(count) = parsed.filter(|value| (2..=MAX_STATED_SCOPES).contains(value)) else {
            continue;
        };
        let after = &lower[start + digits.len()..];
        let window: String = after.chars().take(CARDINALITY_WINDOW).collect();
        if keywords.iter().any(|keyword| window.contains(keyword)) {
            best = Some(count.max(best.unwrap_or(0)));
        }
    }
    best
}

fn is_remaining_only_follow_up(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    (normalized.contains("残り") || normalized.contains("remaining"))
        && explicit_scope_cardinality(content).is_none()
        && count_explicit_blocks(content) < 2
        && !contains_single_scope_request(content)
}

fn is_remaining_count_prefix(before: &str) -> bool {
    let tail: String = before
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let trimmed = tail.trim_end();
    trimmed.ends_with("残り") || trimmed.to_ascii_lowercase().ends_with("remaining")
}

fn count_explicit_blocks(content: &str) -> usize {
    content
        .lines()
        .filter(|line| is_explicit_block(line))
        .count()
}

fn is_explicit_block(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("・")
        || is_numbered_block(trimmed)
}

fn is_numbered_block(trimmed: &str) -> bool {
    let Some(character) = trimmed.chars().next() else {
        return false;
    };
    if !character.is_ascii_digit() {
        return false;
    }
    trimmed
        .char_indices()
        .nth(1)
        .is_some_and(|(index, _)| trimmed[index..].starts_with(". "))
}
