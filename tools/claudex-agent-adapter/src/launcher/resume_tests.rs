#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, path::Path};

    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn extracts_resume_ids_without_confusing_other_flags() {
        assert_eq!(resume_session_id(&args(&["--resume", "session-a"])), Some("session-a".to_owned()));
        assert_eq!(resume_session_id(&args(&["--resume=session-b"])), Some("session-b".to_owned()));
        assert_eq!(resume_session_id(&args(&["-r", "session-c"])), Some("session-c".to_owned()));
        assert_eq!(resume_session_id(&args(&["--continue"])), None);
        assert_eq!(resume_session_id(&args(&["--resume", "--fork-session"])), None);
    }

    #[test]
    fn preserves_resume_id_but_uses_a_new_id_for_forked_sessions() {
        assert_eq!(
            session_id_for_launch(&args(&["--resume", "session-a"]), || "random".to_owned()),
            "session-a"
        );
        assert_eq!(
            session_id_for_launch(
                &args(&["--resume", "session-a", "--fork-session"]),
                || "random".to_owned()
            ),
            "random"
        );
    }

    #[test]
    fn forks_only_resume_histories_that_contain_the_spawn_limit_error() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        let project = root.path().join(".claude/projects/-Users-test-github-com-project");
        fs::create_dir_all(&project).expect("project transcript directory");
        fs::write(
            project.join("session-a.jsonl"),
            "{\"message\":\"Subagent spawn limit reached (200 of 200 agents spawned)\"}\n",
        )
        .expect("limited transcript");
        let prepared = prepare_arguments_with_home(args(&["--resume", "session-a"]), cwd, Some(root.path()), true);
        assert!(prepared.iter().any(|argument| argument == "--fork-session"));

        let clean = prepare_arguments_with_home(args(&["--resume", "session-b"]), cwd, Some(root.path()), true);
        assert!(!clean.iter().any(|argument| argument == "--fork-session"));
        let disabled = prepare_arguments_with_home(args(&["--resume", "session-a"]), cwd, Some(root.path()), false);
        assert!(!disabled.iter().any(|argument| argument == "--fork-session"));
    }
}
