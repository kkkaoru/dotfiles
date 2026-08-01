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
    assert!(ROUTING_INSTRUCTIONS.contains("plugin-qualified"));
    assert!(ROUTING_INSTRUCTIONS.contains(QUALIFIED_PROFILE_NAME));
    assert!(ROUTING_INSTRUCTIONS.contains("never set claudex_model"));
    for invalid in [
        "claudex-xhigh",
        "claudex-max",
        "claudex-gpt",
        "claudex-deepseek",
    ] {
        assert!(!ROUTING_INSTRUCTIONS.contains(invalid));
    }
}

#[test]
fn project_grok_worker_enforces_the_same_nested_contract() {
    let worker = include_str!("../../../../../.claude/agents/claudex-grok.md");
    assert!(worker.contains("model: grok-4.5"));
    assert!(worker.contains("effort: high"));
    assert!(worker.contains("plugin-qualified `grok-native-high-plugin-v3:claudex-high`"));
    assert!(worker.contains("Do not specify a model"));
    assert!(worker.contains("never launch project/global cross-provider"));
    assert!(worker.contains("Do not follow global `selected_workers`"));
    assert!(!worker.contains("exact `claudex_model`"));
    for invalid in [
        "claudex-xhigh",
        "claudex-max",
        "claudex-gpt",
        "claudex-deepseek",
    ] {
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
    let agents = home
        .path()
        .join(".cache/claudex/grok-native-high-plugin-v3/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("claudex-gpt.md"), "stale unsafe shadow").unwrap();
    let plugin = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned()))
        .unwrap()
        .unwrap();

    for alias in UNSAFE_CROSS_PROVIDER_ALIASES {
        assert!(!plugin.join("agents").join(format!("{alias}.md")).exists());
        assert!(REJECT_UNSAFE_AGENT_SCRIPT.contains(alias));
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
    assert!(REJECT_UNSAFE_AGENT_SCRIPT.contains(QUALIFIED_PROFILE_NAME));
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
