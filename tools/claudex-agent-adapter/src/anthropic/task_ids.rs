//! Claude Code Agent task IDs versus Bash-background / other-session nanoids.

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

pub(in crate::anthropic) fn skipped_foreign_task_stop_notice(task_id: &str) -> String {
    format!(
        "Claudex: TaskStop skipped for `{task_id}` — not a stoppable Agent id in this session (background shell or other-session orphan). Use only current Agent ids matching `a` + 16 hex. Already stopped; do not retry."
    )
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
    fn skip_notice_names_the_rejected_id() {
        let notice = skipped_foreign_task_stop_notice("b13mjnjlj");
        assert!(notice.contains("`b13mjnjlj`"));
        assert!(notice.contains("Already stopped"));
        assert!(!notice.contains("No task found"));
    }
}
