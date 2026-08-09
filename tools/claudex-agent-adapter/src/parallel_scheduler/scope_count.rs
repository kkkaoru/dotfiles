use serde_json::Value;

use crate::anthropic::MessagesRequest;

const MAX_STATED_SCOPES: usize = 40;
const CARDINALITY_WINDOW: usize = 24;

pub(crate) fn has_parallel_scope(request: &MessagesRequest) -> bool {
    independent_scope_count(request) >= 2
}

pub(crate) fn independent_scope_count(request: &MessagesRequest) -> usize {
    let user_text = request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .filter(|content| !content.trim_start().starts_with("<task-notification>"))
        .collect::<Vec<_>>();
    let Some(content) = user_text
        .iter()
        .rev()
        .find(|content| !is_remaining_only_follow_up(content))
        .copied()
    else {
        // A reconstructed request without a user turn cannot be classified safely. Keep the
        // existing conservative behavior until the next user turn supplies a scope.
        return 2;
    };
    count_for_content(content)
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

pub(super) fn needs_single_worker(request: &MessagesRequest) -> bool {
    if independent_scope_count(request) >= 2 {
        return false;
    }
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .any(|content| {
            let normalized = content.to_ascii_lowercase();
            [
                "gh ",
                "git ",
                "bash",
                "shell",
                "command",
                "http://",
                "https://",
                "調査",
                "取得",
                "確認",
                "実行",
                "修正",
                "テスト",
                "review",
                "research",
                "investigate",
                "fetch",
                "implement",
                "fix",
                "test",
            ]
            .iter()
            .any(|hint| normalized.contains(hint))
        })
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
