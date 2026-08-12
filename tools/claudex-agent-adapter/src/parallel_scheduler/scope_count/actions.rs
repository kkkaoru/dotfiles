use super::{
    detect::{contains_parallel_intent, contains_single_scope_request, explicit_scope_cardinality},
    filters::{is_negated_or_diagnostic, remove_negative_or_diagnostic_lines},
    MAX_STATED_SCOPES,
};

pub(super) fn declines_delegation_text(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "do not delegate",
        "don't delegate",
        "do not launch",
        "don't launch",
        "never launch",
        "no delegation",
        "stop remaining subagents",
        "stop remaining workers",
        "委譲しない",
        "subagentを起動しない",
        "subagent は不要",
        "subagent不要",
        "残りのsubagentを停止",
        "残りのsubagentを止め",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub(super) fn count_for_content(content: &str) -> usize {
    if declines_delegation_text(content) {
        return 1;
    }
    let semantic = remove_negative_or_diagnostic_lines(content);
    if contains_single_scope_request(&semantic) {
        return 1;
    }
    if let Some(count) = explicit_scope_cardinality(&semantic) {
        return count;
    }
    if let Some(action_count) = clear_action_list_count(&semantic) {
        return action_count;
    }
    if contains_parallel_intent(&semantic) {
        2
    } else {
        1
    }
}

fn clear_action_list_count(content: &str) -> Option<usize> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut best = 0;
    let mut context = "";
    let mut index = 0;
    while index < lines.len() {
        if list_item(lines[index]).is_none() {
            update_list_context(lines[index], &mut context);
            index += 1;
            continue;
        }
        let (entries, next_index) = collect_list_entries(&lines, index);
        index = next_index;
        let top_level = top_level_bodies(&entries);
        let context_is_action = is_action_list_header(context);
        let context_rejects_list = is_non_action_list_header(context);
        let all_items_are_actions = top_level.iter().all(|body| is_action_item(body));
        if top_level.len() >= 2
            && !context_rejects_list
            && (context_is_action || all_items_are_actions)
        {
            best = best.max(top_level.len().min(MAX_STATED_SCOPES));
        }
    }
    (best >= 2).then_some(best)
}

fn top_level_bodies<'a>(entries: &[(usize, &'a str)]) -> Vec<&'a str> {
    let mut base_indent = None;
    entries
        .iter()
        .filter_map(|(indent, body)| {
            if base_indent.is_none_or(|base| *indent <= base) {
                base_indent = Some(*indent);
                Some(*body)
            } else {
                None
            }
        })
        .collect()
}

fn update_list_context<'a>(line: &'a str, context: &mut &'a str) {
    if !line.trim().is_empty() {
        *context = line.trim();
    }
}

fn collect_list_entries<'a>(lines: &[&'a str], mut index: usize) -> (Vec<(usize, &'a str)>, usize) {
    let mut entries = Vec::new();
    while let Some(line) = lines.get(index) {
        if let Some(entry) = list_item(line) {
            entries.push(entry);
            index += 1;
            continue;
        }
        if line.trim().is_empty() || is_indented_continuation(line) {
            index += 1;
            continue;
        }
        break;
    }
    (entries, index)
}

fn list_item(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    for marker in ["- ", "* ", "・"] {
        if let Some(body) = trimmed.strip_prefix(marker) {
            return Some((indent, strip_checkbox(body).trim()));
        }
    }
    let digit_count = trimmed
        .chars()
        .take_while(char::is_ascii_digit)
        .map(char::len_utf8)
        .sum::<usize>();
    if digit_count == 0 {
        return None;
    }
    trimmed[digit_count..]
        .strip_prefix(". ")
        .map(|body| (indent, strip_checkbox(body).trim()))
}

fn strip_checkbox(body: &str) -> &str {
    ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|prefix| body.strip_prefix(prefix))
        .unwrap_or(body)
}

fn is_indented_continuation(line: &str) -> bool {
    !line.trim().is_empty() && line.len() > line.trim_start().len()
}

fn is_action_list_header(header: &str) -> bool {
    if is_non_action_list_header(header) {
        return false;
    }
    let lower = header.to_ascii_lowercase();
    [
        "task",
        "workstream",
        "independent scope",
        "scope",
        "request",
        "input",
        "please",
        "run ",
        "review",
        "investigate",
        "implement",
        "fix",
        "test",
        "verify",
        "analyze",
        "audit",
        "build",
        "recover",
        "依頼",
        "作業",
        "分担",
        "調査",
        "実装",
        "修正",
        "検証",
        "分析",
        "対応",
        "以下を",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_non_action_list_header(header: &str) -> bool {
    let lower = header.to_ascii_lowercase();
    [
        "acceptance criteria",
        "constraint",
        "non-goal",
        "example:",
        "examples:",
        "output",
        "log:",
        "logs:",
        "requirement",
        "検収条件",
        "受入条件",
        "制約",
        "非目標",
        "対象外",
        "例:",
        "例：",
        "出力",
        "ログ",
        "要件",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_action_item(item: &str) -> bool {
    if is_negated_or_diagnostic(item) {
        return false;
    }
    let lower = item.to_ascii_lowercase();
    let first_word = lower
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
        .find(|word| !word.is_empty())
        .unwrap_or("");
    matches!(
        first_word,
        "implement"
            | "fix"
            | "review"
            | "investigate"
            | "test"
            | "verify"
            | "update"
            | "add"
            | "remove"
            | "refactor"
            | "analyze"
            | "audit"
            | "build"
            | "recover"
            | "inspect"
            | "trace"
            | "diagnose"
            | "document"
            | "compare"
    ) || [
        "実装",
        "修正",
        "レビュー",
        "調査",
        "テスト",
        "検証",
        "更新",
        "追加",
        "削除",
        "分析",
        "監査",
        "復旧",
        "確認",
        "比較",
        "変換",
        "診断",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub(super) fn is_atomic_lookup(content: &str) -> bool {
    let semantic = remove_negative_or_diagnostic_lines(content);
    if contains_single_scope_request(&semantic) {
        return true;
    }
    let trimmed = semantic.trim();
    let lower = trimmed.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    matches!(first, "gh" | "git" | "curl" | "wget")
        || ((lower.starts_with("http://") || lower.starts_with("https://"))
            && !trimmed.contains('\n')
            && trimmed.split_whitespace().count() <= 3)
}
