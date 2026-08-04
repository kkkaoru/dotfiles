use std::process::Command;

pub(super) fn another_resume_launcher_is_active(session_id: &str) -> anyhow::Result<bool> {
    let output = match Command::new("ps").args(["-axo", "pid=,command="]).output() {
        Ok(output) => output,
        Err(_) => return Ok(false),
    };
    let current_pid = std::process::id();
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| process_line_matches_resume_launcher(line, session_id))
        .any(|pid| pid != current_pid))
}

/// True when any interactive `claudex-agent-adapter launch` parent is alive.
///
/// Handover must not SIGTERM the shared `serve` daemon while a TUI session still
/// holds that parent: killing the listener mid-session aborts Claude Code and
/// forces resume to restart from compaction.
pub(super) fn any_launch_is_active() -> bool {
    let output = match Command::new("ps").args(["-axo", "pid=,command="]).output() {
        Ok(output) => output,
        Err(_) => return false,
    };
    let current_pid = std::process::id();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(process_line_matches_launch)
        .any(|pid| pid != current_pid)
}

fn process_line_matches_launch(line: &str) -> Option<u32> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let executable = fields.next()?;
    if executable.rsplit('/').next()? != "claudex-agent-adapter" {
        return None;
    }
    (fields.next() == Some("launch")).then_some(pid)
}

fn process_line_matches_resume_launcher(line: &str, session_id: &str) -> Option<u32> {
    let pid = process_line_matches_launch(line)?;
    let arguments = line.split_whitespace().skip(2).collect::<Vec<_>>();
    let resume_equals = format!("--resume={session_id}");
    let short_resume_equals = format!("-r={session_id}");
    let has_resume = arguments
        .windows(2)
        .any(|pair| (pair[0] == "--resume" || pair[0] == "-r") && pair[1] == session_id);
    (has_resume
        || arguments
            .iter()
            .any(|argument| *argument == resume_equals || *argument == short_resume_equals))
    .then_some(pid)
}

#[cfg(test)]
mod tests {
    use super::{process_line_matches_launch, process_line_matches_resume_launcher};

    #[test]
    fn recognizes_launch_parents_without_confusing_them_for_serve() {
        assert_eq!(
            process_line_matches_launch(
                "42 /Users/test/.local/bin/claudex-agent-adapter launch --model opus"
            ),
            Some(42)
        );
        assert_eq!(
            process_line_matches_launch(
                "42 /Users/test/.local/bin/claudex-agent-adapter serve --model opus"
            ),
            None
        );
    }

    #[test]
    fn recognizes_only_other_claudex_launchers_for_the_same_resume() {
        let line = "42 /Users/test/.local/bin/claudex-agent-adapter launch --resume session-a";
        assert_eq!(
            process_line_matches_resume_launcher(line, "session-a"),
            Some(42)
        );
        assert_eq!(
            process_line_matches_resume_launcher(line, "session-b"),
            None
        );
        assert_eq!(
            process_line_matches_resume_launcher(
                "42 /Users/test/.local/bin/claudex-agent-adapter serve --resume session-a",
                "session-a"
            ),
            None
        );
        assert_eq!(
            process_line_matches_resume_launcher(
                "42 /bin/zsh -lc claudex-agent-adapter launch --resume session-a",
                "session-a"
            ),
            None
        );
    }
}
