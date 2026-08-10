#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, path::Path};

    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn prepare(values: &[&str], cwd: &Path, home: &Path, auto_fork: bool) -> Vec<OsString> {
        prepare_arguments_with_home(args(values), cwd, Some(home), None, auto_fork)
    }

    fn write_transcript(home: &Path, cwd: &Path, session_id: &str, body: &str) {
        let project = home.join(".claude/projects").join(project_dir_name(cwd));
        fs::create_dir_all(&project).expect("project transcript directory");
        fs::write(project.join(format!("{session_id}.jsonl")), body).expect("transcript");
    }

    #[test]
    fn extracts_resume_ids_without_confusing_other_flags() {
        assert_eq!(
            resume_session_id(&args(&["--resume", "session-a"])),
            Some("session-a".to_owned())
        );
        assert_eq!(
            resume_session_id(&args(&["--resume=session-b"])),
            Some("session-b".to_owned())
        );
        assert_eq!(
            resume_session_id(&args(&["-r", "session-c"])),
            Some("session-c".to_owned())
        );
        assert_eq!(resume_session_id(&args(&["--continue"])), None);
        assert_eq!(
            resume_session_id(&args(&["--resume", "--fork-session"])),
            None
        );
    }

    #[test]
    fn preserves_resume_id_but_uses_a_new_id_for_forked_sessions() {
        assert_eq!(
            session_id_for_launch(&args(&["--resume", "session-a"]), || "random".to_owned()),
            "session-a"
        );
        assert_eq!(
            session_id_for_launch(&args(&["--resume", "session-a", "--fork-session"]), || {
                "random".to_owned()
            }),
            "random"
        );
    }

    #[test]
    fn only_an_unforked_explicit_resume_claims_a_session_lock() {
        assert_eq!(
            session_lock_id(&args(&["--resume", "session-a"])),
            Some("session-a".to_owned())
        );
        assert_eq!(
            session_lock_id(&args(&["--resume", "session-a", "--fork-session"])),
            None
        );
        assert_eq!(session_lock_id(&args(&["--continue"])), None);
    }

    #[test]
    fn forks_only_resume_histories_that_contain_the_spawn_limit_error() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        write_transcript(
            root.path(),
            cwd,
            "session-a",
            "{\"message\":\"Subagent spawn limit reached (200 of 200 agents spawned)\"}\n",
        );
        let prepared = prepare(&["--resume", "session-a"], cwd, root.path(), true);
        assert!(prepared.iter().any(|argument| argument == "--fork-session"));

        let clean = prepare(&["--resume", "session-b"], cwd, root.path(), true);
        assert!(!clean.iter().any(|argument| argument == "--fork-session"));
        let disabled = prepare(&["--resume", "session-a"], cwd, root.path(), false);
        assert!(!disabled.iter().any(|argument| argument == "--fork-session"));
    }

    #[test]
    fn restores_display_name_from_slug_when_legacy_orchestrator_agent_setting_remains() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        write_transcript(
            root.path(),
            cwd,
            "session-legacy",
            concat!(
                "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
                "{\"type\":\"attachment\",\"slug\":\"humming-sprouting-scroll\"}\n",
            ),
        );

        let prepared = prepare(&["--resume", "session-legacy"], cwd, root.path(), false);
        assert!(
            prepared
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "humming-sprouting-scroll" })
        );
    }

    #[test]
    fn prefers_custom_title_and_skips_when_display_name_already_restored() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        write_transcript(
            root.path(),
            cwd,
            "session-titled",
            concat!(
                "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"already-renamed\"}\n",
                "{\"type\":\"attachment\",\"slug\":\"humming-sprouting-scroll\"}\n",
            ),
        );

        let prepared = prepare(&["--resume", "session-titled"], cwd, root.path(), false);
        assert!(!prepared.iter().any(|argument| argument == "--name"));
    }

    #[test]
    fn falls_back_to_cwd_basename_when_legacy_session_has_no_slug() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/avita-platform");
        write_transcript(
            root.path(),
            cwd,
            "session-bare",
            "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
        );

        let prepared = prepare(&["--resume", "session-bare"], cwd, root.path(), false);
        assert!(
            prepared
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "avita-platform" })
        );
    }

    #[test]
    fn leaves_clean_and_explicit_sessions_untouched() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        write_transcript(
            root.path(),
            cwd,
            "session-clean",
            "{\"type\":\"attachment\",\"slug\":\"fresh-session\"}\n",
        );
        write_transcript(
            root.path(),
            cwd,
            "session-legacy",
            concat!(
                "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
                "{\"type\":\"attachment\",\"slug\":\"humming-sprouting-scroll\"}\n",
            ),
        );

        let clean = prepare(&["--resume", "session-clean"], cwd, root.path(), false);
        assert!(!clean.iter().any(|argument| argument == "--name"));

        let named = prepare(
            &["--resume", "session-legacy", "--name", "user-chosen"],
            cwd,
            root.path(),
            false,
        );
        assert_eq!(
            named
                .iter()
                .filter(|argument| *argument == "--name")
                .count(),
            1
        );
        assert!(
            named
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "user-chosen" })
        );

        let agent = prepare(
            &[
                "--resume",
                "session-legacy",
                "--agent",
                "claudex-orchestrator",
            ],
            cwd,
            root.path(),
            false,
        );
        assert!(!agent.iter().any(|argument| argument == "--name"));

        let named_equals = prepare(
            &["--resume", "session-legacy", "--name=user-chosen"],
            cwd,
            root.path(),
            false,
        );
        assert!(
            !named_equals
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "humming-sprouting-scroll" })
        );

        let agent_equals = prepare(
            &["--resume", "session-legacy", "--agent=claudex-orchestrator"],
            cwd,
            root.path(),
            false,
        );
        assert!(!agent_equals.iter().any(|argument| argument == "--name"));

        let empty_equals = prepare(
            &["--resume", "session-legacy", "--name="],
            cwd,
            root.path(),
            false,
        );
        assert!(
            empty_equals
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "humming-sprouting-scroll" })
        );

        let dash_value = prepare(
            &["--resume", "session-legacy", "--name", "--fork-session"],
            cwd,
            root.path(),
            false,
        );
        assert!(
            dash_value
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "humming-sprouting-scroll" })
        );
    }

    #[test]
    fn skips_non_utf8_flags_invalid_transcript_lines_and_missing_config_dir_files() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        write_transcript(
            root.path(),
            cwd,
            "session-messy",
            concat!(
                "{\"type\":\"agent-setting\" not-json\n",
                "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
                "{\"type\":\"attachment\",\"slug\":\"messy-scroll\"}\n",
            ),
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut values = args(&["--resume", "session-messy"]);
            values.push(OsString::from_vec(vec![0xff, 0xfe]));
            values.push(OsString::from("--name=user-chosen"));
            let prepared = prepare_arguments_with_home(values, cwd, Some(root.path()), None, false);
            assert!(
                !prepared
                    .windows(2)
                    .any(|window| { window[0] == "--name" && window[1] == "messy-scroll" })
            );
        }

        let config_dir = root.path().join("isolated-config");
        fs::create_dir_all(config_dir.join("projects")).expect("empty config projects");
        let prepared = prepare_arguments_with_home(
            args(&["--resume", "session-messy"]),
            cwd,
            Some(root.path()),
            Some(config_dir.as_path()),
            false,
        );
        assert!(
            prepared
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "messy-scroll" })
        );

        let no_resume = prepare(&["--continue"], cwd, root.path(), true);
        assert!(
            !no_resume
                .iter()
                .any(|argument| argument == "--fork-session")
        );
        assert!(!no_resume.iter().any(|argument| argument == "--name"));
    }

    #[test]
    fn reads_transcripts_from_claude_config_dir_when_present() {
        let root = tempfile::tempdir().expect("resume fixture");
        let cwd = Path::new("/Users/test/github.com/project");
        let config_dir = root.path().join("isolated-config");
        let project = config_dir.join("projects").join(project_dir_name(cwd));
        fs::create_dir_all(&project).expect("isolated project");
        fs::write(
            project.join("session-isolated.jsonl"),
            concat!(
                "{\"type\":\"agent-setting\",\"agentSetting\":\"claudex-orchestrator\"}\n",
                "{\"type\":\"agent-name\",\"agentName\":\"preserved-session-name\"}\n",
            ),
        )
        .expect("isolated transcript");

        let prepared = prepare_arguments_with_home(
            args(&["--resume", "session-isolated"]),
            cwd,
            Some(root.path()),
            Some(config_dir.as_path()),
            false,
        );
        assert!(
            prepared
                .windows(2)
                .any(|window| { window[0] == "--name" && window[1] == "preserved-session-name" })
        );
    }
}
