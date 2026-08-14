use std::ffi::OsStr;

use super::{
    PROFILE_EFFORT, PROFILE_NAME, QUALIFIED_PROFILE_NAME, REJECT_UNSAFE_AGENT_SCRIPT,
    ROUTING_INSTRUCTIONS, UNSAFE_CROSS_PROVIDER_ALIASES, hooks_json, prepare, prepare_with,
    profile, write_if_changed,
};

#[test]
fn profile_is_provider_local_model_inheriting_high() {
    let profile = profile(PROFILE_NAME);
    assert_eq!(PROFILE_NAME, "claudex-high");
    assert_eq!(PROFILE_EFFORT, "high");
    assert!(profile.contains("effort: high"));
    assert!(!profile.contains("\nmodel:"));
    assert!(ROUTING_INSTRUCTIONS.contains("Agent or Task"));
    assert!(ROUTING_INSTRUCTIONS.contains("selected_workers"));
    assert!(ROUTING_INSTRUCTIONS.contains("run_in_background=true"));
    assert!(ROUTING_INSTRUCTIONS.contains("spawn_subagent"));
    assert!(ROUTING_INSTRUCTIONS.contains("bridges it to Claude Code Agent"));
    assert!(ROUTING_INSTRUCTIONS.contains("not a delegated worker"));
    assert!(!ROUTING_INSTRUCTIONS.contains(QUALIFIED_PROFILE_NAME));
    for invalid in ["claudex-xhigh", "claudex-max"] {
        assert!(!ROUTING_INSTRUCTIONS.contains(invalid));
    }
}

#[test]
fn project_grok_worker_enforces_the_same_nested_contract() {
    let worker = include_str!("../../../../../.claude/agents/claudex-grok.md");
    assert!(worker.contains("model: grok-4.6"));
    assert!(worker.contains("effort: high"));
    assert!(worker.contains("never Grok `spawn_subagent`"));
    assert!(worker.contains("subagent_type: claudex-grok"));
    assert!(worker.contains("claudex_model: grok-4.6"));
    assert!(worker.contains("run_in_background: true"));
    assert!(!worker.contains("grok-native-high-plugin-v3:claudex-high"));
    for invalid in ["claudex-xhigh", "claudex-max"] {
        assert!(!worker.contains(invalid));
    }
}

#[test]
fn custom_program_does_not_receive_builtin_plugin() {
    assert_eq!(prepare(OsStr::new("grok-acp-mock")).unwrap(), None);

    let plugin = tempfile::tempdir().unwrap();
    assert_eq!(
        prepare_with(OsStr::new("grok"), Some(plugin.path().to_owned()), None).unwrap(),
        Some(plugin.path().to_owned())
    );
    assert!(prepare_with(OsStr::new("grok"), None, None).is_err());
}

#[test]
fn prepares_and_reuses_only_the_builtin_high_profile() {
    let home = tempfile::tempdir().unwrap();
    let plugin = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned()))
        .unwrap()
        .unwrap();
    assert!(plugin.join("agents/claudex-high.md").is_file());
    for invalid in [
        "claudex-low.md",
        "claudex-medium.md",
        "claudex-xhigh.md",
        "claudex-max.md",
    ] {
        assert!(!plugin.join("agents").join(invalid).exists());
    }
    assert_eq!(
        prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned())).unwrap(),
        Some(plugin)
    );
}

#[test]
fn rejects_cross_provider_aliases_before_they_reach_the_grok_api() {
    let home = tempfile::tempdir().unwrap();
    let stale_hook = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3/hooks/hooks.json");
    std::fs::create_dir_all(stale_hook.parent().unwrap()).unwrap();
    std::fs::write(&stale_hook, "stale process hook").unwrap();
    let agents = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("claudex-gpt.md"), "stale unsafe shadow").unwrap();
    let plugin = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned()))
        .unwrap()
        .unwrap();
    assert!(!stale_hook.exists());

    for alias in UNSAFE_CROSS_PROVIDER_ALIASES {
        assert!(!plugin.join("agents").join(format!("{alias}.md")).exists());
    }
    let guard = plugin.join("bin/reject-cross-provider-agent.sh");
    let hook = std::fs::read_to_string(home.path().join(".grok/hooks/claudex-agent-adapter.json"))
        .unwrap();
    assert_eq!(hook, hooks_json(&guard).unwrap());
    assert_eq!(
        std::fs::read_to_string(plugin.join("bin/reject-cross-provider-agent.sh")).unwrap(),
        REJECT_UNSAFE_AGENT_SCRIPT
    );
    assert!(hook.contains("PreToolUse"));
    assert!(hook.contains("^spawn_subagent$"));
    assert!(REJECT_UNSAFE_AGENT_SCRIPT.contains("CLAUDEX_GROK_ACP"));
    assert!(REJECT_UNSAFE_AGENT_SCRIPT.contains(r#"{"decision":"allow"}"#));
    assert!(!REJECT_UNSAFE_AGENT_SCRIPT.contains(QUALIFIED_PROFILE_NAME));
}

#[test]
fn rewrites_existing_builtin_plugin_files_and_removes_the_stale_hook() {
    let home = tempfile::tempdir().unwrap();
    let root = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3");
    std::fs::create_dir_all(root.join("agents")).unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::create_dir_all(home.path().join(".grok/hooks")).unwrap();
    std::fs::write(root.join("agents/claudex-high.md"), "old profile").unwrap();
    std::fs::write(root.join("bin/reject-cross-provider-agent.sh"), "old guard").unwrap();
    std::fs::write(root.join("hooks/hooks.json"), "old process hook").unwrap();
    std::fs::write(
        home.path().join(".grok/hooks/claudex-agent-adapter.json"),
        "old user hook",
    )
    .unwrap();

    let plugin = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned()))
        .unwrap()
        .unwrap();
    assert_eq!(plugin, root);
    assert!(!root.join("hooks/hooks.json").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("agents/claudex-high.md")).unwrap(),
        profile(PROFILE_NAME)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("bin/reject-cross-provider-agent.sh")).unwrap(),
        REJECT_UNSAFE_AGENT_SCRIPT
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join(".grok/hooks/claudex-agent-adapter.json"))
            .unwrap(),
        hooks_json(&root.join("bin/reject-cross-provider-agent.sh")).unwrap()
    );
}

#[test]
fn writes_profiles_only_when_needed_and_reports_write_failures() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile.md");
    write_if_changed(profile.clone(), "content").unwrap();
    write_if_changed(profile, "content").unwrap();

    let error = write_if_changed(root.path().join("missing/profile.md"), "content").unwrap_err();
    assert!(error.to_string().contains("write"));
}

#[test]
fn reports_stale_shadow_removal_failures_with_the_alias_path() {
    let home = tempfile::tempdir().unwrap();
    let agents = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::create_dir(agents.join("claudex-gpt.md")).unwrap();

    let error = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned())).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("remove stale Grok shadow"));
    assert!(message.contains("claudex-gpt.md"));
}

#[test]
fn reports_user_hook_write_failures_after_preparing_the_cache() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".grok/hooks")).unwrap();
    std::fs::create_dir(home.path().join(".grok/hooks/claudex-agent-adapter.json")).unwrap();

    let error = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned())).unwrap_err();
    assert!(error.to_string().contains("claudex-agent-adapter.json"));
}
