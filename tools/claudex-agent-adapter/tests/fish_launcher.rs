use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fish_launcher_uses_the_shared_provider_config() {
    let home = tempfile::tempdir().expect("temporary launcher home");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1}",
    )
    .expect("Grok config");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'CLAUDEX_ACTIVE=%s\\n' \"${CLAUDEX_ACTIVE:-}\"\nprintf '%s\\n' \"$@\"\n",
    )
    .expect("fake adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("fake adapter metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("executable fake adapter");

    let function = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.config/fish/functions/claudex.fish");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(arguments.contains("--provider-config\n"));
    assert!(arguments.contains("CLAUDEX_ACTIVE=1\n"));
    assert!(arguments.contains(".config/claudex/providers.json\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\nsmoke\n"));

    assert_no_argument_launch(&function, &home);

    let alternate = home.path().join("alternate-providers.json");
    fs::write(&alternate, "{\"version\":1}").expect("alternate config");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex override-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_PROVIDER_CONFIG", &alternate)
        .env("CLAUDEX_MODEL", "vendor-model")
        .output()
        .expect("run Copilot ACP fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 override arguments");
    assert!(arguments.contains(&format!("--provider-config\n{}\n", alternate.display())));
    assert!(arguments.contains("--model\nvendor-model\n"));
    assert!(!arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\noverride-smoke\n"));

    assert_explicit_agent_is_preserved(&function, &home);
    assert_routing_marker_is_scoped_to_claudex(&function, &home);
}

fn assert_explicit_agent_is_preserved(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex --agent custom-subagent smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .output()
        .expect("run explicit-agent fish launcher");
    assert!(output.status.success());
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 explicit-agent arguments");
    assert_eq!(arguments.matches("--agent\n").count(), 1);
    assert!(arguments.ends_with("--\n--agent\ncustom-subagent\nsmoke\n"));
}

fn assert_routing_marker_is_scoped_to_claudex(
    function: &std::path::Path,
    home: &tempfile::TempDir,
) {
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex marker-smoke; if set -q CLAUDEX_ACTIVE; echo leaked; end",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .output()
        .expect("run routing marker smoke");
    assert!(output.status.success());
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 marker arguments");
    assert!(!arguments.contains("leaked"));
}

#[test]
fn provider_workers_fix_the_models_from_the_shared_config() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".config/claudex/providers.json")).expect("provider config"),
    )
    .expect("valid provider config");
    let configured = config["providers"]
        .as_array()
        .expect("configured providers")
        .iter()
        .map(|provider| (&provider["agent"], &provider["defaultModel"]))
        .chain(std::iter::once((
            &config["fallback"]["agent"],
            &config["fallback"]["model"],
        )));
    for (agent, model) in configured {
        let name = format!("{}.md", agent.as_str().expect("worker agent"));
        let model = model.as_str().expect("worker model");
        let definition = fs::read_to_string(root.join(".claude/agents").join(&name))
            .expect("provider worker definition");
        let expected = format!("model: {model}");
        assert!(
            definition.lines().any(|line| line == expected),
            "{name} must match the shared provider model"
        );
    }

    assert_qwen_runtime_is_bounded(&root, &config);
    assert_provider_children_skip_claude_session_hook(&root);
}

#[test]
fn every_subagent_inherits_the_main_tool_and_permission_context() {
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/agents");
    for entry in fs::read_dir(agents).expect("Claude agent definitions") {
        let path = entry.expect("agent directory entry").path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("md") {
            continue;
        }
        let definition = fs::read_to_string(&path).expect("Claude agent definition");
        let frontmatter = definition
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .collect::<Vec<_>>();
        for restricted_field in ["tools:", "disallowedTools:", "permissionMode:"] {
            assert!(
                !frontmatter
                    .iter()
                    .any(|line| line.starts_with(restricted_field)),
                "{} must inherit instead of setting {restricted_field}",
                path.display()
            );
        }
        assert!(
            definition.contains("main session's complete tool set and permission context"),
            "{} must state the permission inheritance contract",
            path.display()
        );
    }
}

fn assert_qwen_runtime_is_bounded(root: &std::path::Path, config: &serde_json::Value) {
    let qwen = config["providers"]
        .as_array()
        .expect("configured providers")
        .iter()
        .find(|provider| provider["id"] == "qwen")
        .expect("Qwen provider");
    assert_eq!(qwen["acp"]["program"], "/usr/bin/env");
    assert_eq!(
        qwen["acp"]["arguments"],
        serde_json::json!([
            "QWEN_WEB_FETCH_PROCESSING_TIMEOUT_MS=8000",
            "qwen",
            "--acp",
            "--approval-mode",
            "yolo",
            "--model",
            "{model}"
        ])
    );

    let agent = fs::read_to_string(root.join(".claude/agents/claudex-qwen.md"))
        .expect("Qwen worker definition");
    let policy_text = agent.split_whitespace().collect::<Vec<_>>().join(" ");
    for policy in [
        "more than one `web_fetch` in a tool batch",
        "more than two `web_fetch` calls per delegated task",
        "Never retry the same or a substantially equivalent URL",
    ] {
        assert!(
            policy_text.contains(policy),
            "missing Qwen research policy: {policy}"
        );
    }
}

fn assert_provider_children_skip_claude_session_hook(root: &std::path::Path) {
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".claude/settings.json")).expect("Claude settings"),
    )
    .expect("valid Claude settings");
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .expect("SessionStart command");
    let hook = command.find("herdr-agent-state.sh").expect("Herdr hook");
    for guard in [
        "CLAUDEX_NONINTERACTIVE_CHILD",
        "CLAUDEX_GROK_ACP",
        "CLAUDEX_PROVIDER_ACP",
    ] {
        assert!(
            command.find(guard).is_some_and(|index| index < hook),
            "{guard} must be checked before the Herdr hook reads stdin"
        );
        let output = Command::new("sh")
            .args(["-c", command])
            .env(guard, "1")
            .env("HOME", "/path/that/must/not/be-read")
            .output()
            .expect("run guarded SessionStart hook");
        assert!(output.status.success());
    }
}

fn assert_no_argument_launch(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = Command::new("fish")
        .args(["-c", &format!("source '{}'; claudex", function.display())])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run no-argument fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\n"));
}
