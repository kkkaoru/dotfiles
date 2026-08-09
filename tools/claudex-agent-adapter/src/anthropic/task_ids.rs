//! Claude Code Agent task IDs versus Bash-background / other-session nanoids.

use serde_json::Value;

/// Live Claude Code Agent/TaskStop targets are `a` + 16 hex digits.
///
/// Background Bash (`b13mjnjlj`, `bjh859kgm`) and other-session orphans use a
/// short nanoid. TaskStop on those IDs yields `No task found` in the TUI.
pub(in crate::anthropic) fn is_claude_code_agent_task_id(task_id: &str) -> bool {
    let id = task_id.trim();
    let mut chars = id.chars();
    matches!(chars.next(), Some('a' | 'A'))
        && chars.all(|character| character.is_ascii_hexdigit())
        && id.len() == 17
}

pub(in crate::anthropic) fn is_task_stop_tool_name(name: &str) -> bool {
    matches!(name, "TaskStop" | "StopTask" | "Stop Task")
}

pub(in crate::anthropic) fn is_task_output_tool_name(name: &str) -> bool {
    matches!(name, "TaskOutput" | "TaskGet")
}

pub(in crate::anthropic) fn task_output_id(arguments: &Value) -> &str {
    arguments
        .get("task_id")
        .and_then(Value::as_str)
        .or_else(|| arguments.get("id").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
}

pub(in crate::anthropic) fn skipped_foreign_task_stop_notice(task_id: &str) -> String {
    format!(
        "Claudex: TaskStop skipped for `{task_id}` — not a stoppable Agent id in this session (background shell or other-session orphan). Use only current Agent ids matching `a` + 16 hex. Already stopped; do not retry."
    )
}

pub(in crate::anthropic) fn unknown_task_output_notice(
    task_id: &str,
    live_ids: &[String],
) -> String {
    let live = live_ids
        .iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Claudex: TaskOutput skipped for `{task_id}` — not an active Agent task in this session. Live task ids: {live}. Retry TaskOutput with one of those ids; do not treat this as completed output."
    )
}

pub(in crate::anthropic) fn stale_task_output_notice(
    arguments: &Value,
    live_ids: &[String],
) -> Option<String> {
    let task_id = task_output_id(arguments);
    if task_id.is_empty() || live_ids.is_empty() {
        return None;
    }
    if live_ids.iter().any(|id| id.eq_ignore_ascii_case(task_id)) {
        return None;
    }
    Some(unknown_task_output_notice(task_id, live_ids))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_live_claude_code_agent_ids_only() {
        assert!(is_claude_code_agent_task_id("a4b2412c427ee5327"));
        assert!(is_claude_code_agent_task_id("A886D9A7AB578419C"));
        assert!(is_claude_code_agent_task_id(" a5893acb44c266fdf "));
        assert!(!is_claude_code_agent_task_id("b13mjnjlj"));
        assert!(!is_claude_code_agent_task_id("bfn35ry3f"));
        assert!(!is_claude_code_agent_task_id("bjh859kgm"));
        assert!(!is_claude_code_agent_task_id("a4b2412c427ee532"));
        assert!(!is_claude_code_agent_task_id("a4b2412c427ee53270"));
        assert!(!is_claude_code_agent_task_id("agent-profile-7"));
        assert!(!is_claude_code_agent_task_id(""));
    }

    #[test]
    fn recognizes_task_stop_tool_aliases() {
        assert!(is_task_stop_tool_name("TaskStop"));
        assert!(is_task_stop_tool_name("StopTask"));
        assert!(is_task_stop_tool_name("Stop Task"));
        assert!(!is_task_stop_tool_name("TaskOutput"));
        assert!(!is_task_stop_tool_name("Agent"));
    }

    #[test]
    fn recognizes_task_output_tool_aliases() {
        assert!(is_task_output_tool_name("TaskOutput"));
        assert!(is_task_output_tool_name("TaskGet"));
        assert!(!is_task_output_tool_name("TaskStop"));
        assert!(!is_task_output_tool_name("Agent"));
    }

    #[test]
    fn skip_notice_names_the_rejected_id() {
        let notice = skipped_foreign_task_stop_notice("b13mjnjlj");
        assert!(notice.contains("`b13mjnjlj`"));
        assert!(notice.contains("Already stopped"));
        assert!(!notice.contains("No task found"));
    }

    #[test]
    fn skips_stale_task_output_when_live_agents_are_known() {
        let live = vec![
            "a4496564387a2561f".to_owned(),
            "a906c77ad60469b0a".to_owned(),
            "a9151d416c226141d".to_owned(),
        ];
        let notice = stale_task_output_notice(
            &serde_json::json!({"task_id":"a3d7f2ca50556c9e5","block":false}),
            &live,
        )
        .expect("stale TaskOutput id");
        assert!(notice.contains("`a3d7f2ca50556c9e5`"));
        assert!(notice.contains("`a4496564387a2561f`"));
        assert!(notice.contains("do not treat this as completed output"));
        assert!(
            stale_task_output_notice(&serde_json::json!({"task_id":"a4496564387a2561f"}), &live,)
                .is_none()
        );
        assert!(
            stale_task_output_notice(&serde_json::json!({"task_id":"a3d7f2ca50556c9e5"}), &[],)
                .is_none()
        );
    }
}
