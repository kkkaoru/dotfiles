use super::{CARDINALITY_WINDOW, MAX_STATED_SCOPES};

pub(super) fn contains_substantive_verb(content: &str) -> bool {
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

pub(super) fn contains_single_scope_request(content: &str) -> bool {
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

pub(super) fn contains_parallel_intent(content: &str) -> bool {
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

pub(super) fn explicit_scope_cardinality(content: &str) -> Option<usize> {
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

pub(super) fn is_remaining_only_follow_up(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    (normalized.contains("残り") || normalized.contains("remaining"))
        && explicit_scope_cardinality(content).is_none()
        && count_explicit_blocks(content) < 2
        && !contains_single_scope_request(content)
}

pub(super) fn is_remaining_count_prefix(before: &str) -> bool {
    let trimmed = before.trim_end();
    trimmed.ends_with("残り") || trimmed.to_ascii_lowercase().ends_with("remaining")
}

pub(super) fn count_explicit_blocks(content: &str) -> usize {
    content
        .lines()
        .filter(|line| is_explicit_block(line))
        .count()
}

pub(super) fn is_explicit_block(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("・")
        || is_numbered_block(trimmed)
}

pub(super) fn is_numbered_block(trimmed: &str) -> bool {
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
