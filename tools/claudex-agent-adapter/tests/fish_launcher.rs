use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fish_launcher_uses_the_shared_provider_config() {
    let home = shared_provider_fixture();
    let function = launcher_function();
    assert_shared_provider_default(&function, &home);
    assert_no_argument_launch(&function, &home);
    assert_explicit_override(&function, &home);
    assert_explicit_agent_is_preserved(&function, &home);
    assert_routing_marker_is_scoped_to_claudex(&function, &home);
    assert_local_defaults(&function, &home);
}

fn shared_provider_fixture() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("temporary launcher home");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::create_dir_all(home.path().join(".claude")).expect("settings directory");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1,\"mainProviders\":[\"p\"],\"providers\":[{\"id\":\"p\",\"agent\":\"worker\",\"defaultModel\":\"model\",\"subagentModel\":\"worker-model\",\"effort\":\"high\",\"backend\":\"codex-app-server\"}],\"fallback\":{\"agent\":\"fallback\",\"model\":\"sonnet\",\"effort\":\"high\"}}",
    )
    .expect("Grok config");
    fs::write(
        home.path().join(".claude/settings.json"),
        "{\"model\":\"sonnet[1m]\",\"effortLevel\":\"high\"}",
    )
    .expect("temporary settings file");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'CLAUDEX_ACTIVE=%s\\n' \"${CLAUDEX_ACTIVE:-}\"\nprintf 'CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=%s\\n' \"${CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY:-}\"\nprintf 'CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=%s\\n' \"${CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS:-}\"\nprintf 'CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=%s\\n' \"${CLAUDE_CODE_ALWAYS_ENABLE_EFFORT:-}\"\nprintf '%s\\n' \"$@\"\n",
    )
    .expect("fake adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("fake adapter metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("executable fake adapter");
    home
}

fn launcher_function() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.config/fish/functions/claudex.fish")
}

fn assert_shared_provider_default(function: &std::path::Path, home: &tempfile::TempDir) {
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
    assert!(arguments.contains("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1\n"));
    assert!(arguments.contains("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=40\n"));
    assert!(arguments.contains(".config/claudex/providers.json\n"));
    assert!(arguments.contains("--model\nmodel\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains("--subscription-max-processes\n20\n"));
    assert!(arguments.contains("--allowedTools\nWebSearch,WebFetch\n"));
    assert!(arguments.ends_with(
        "--\n--agent\nclaudex-orchestrator\n--allowedTools\nWebSearch,WebFetch\nsmoke\n"
    ));
    assert!(String::from_utf8_lossy(&output.stderr).contains("settings sonnet[1m], high"));
}

fn assert_explicit_override(function: &std::path::Path, home: &tempfile::TempDir) {
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
    assert!(arguments.contains("--effort\nhigh\n"));
    assert!(arguments.contains("--allowedTools\nWebSearch,WebFetch\n"));
    assert!(
        arguments
            .ends_with(
                "--\n--agent\nclaudex-orchestrator\n--effort\nhigh\n--allowedTools\nWebSearch,WebFetch\noverride-smoke\n"
            )
    );
}

fn assert_local_defaults(function: &std::path::Path, home: &tempfile::TempDir) {
    fs::write(
        home.path().join(".config/claudex/defaults.local.json"),
        "{\"version\":1,\"source\":\"explicit\",\"model\":\"gpt-local\",\"effort\":\"low\"}",
    )
    .expect("explicit defaults");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex local-default-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_EFFORT")
        .output()
        .expect("run explicit defaults fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 explicit defaults arguments");
    assert!(arguments.contains("--model\ngpt-local\n"));
    assert!(arguments.contains("--effort\nlow\n"));
    assert!(!arguments.contains("--inherit-claude-model\n"));

    fs::write(
        home.path().join(".config/claudex/defaults.local.json"),
        "{\"version\":1,\"source\":\"unsupported\",\"model\":\"gpt-local\",\"effort\":\"low\"}",
    )
    .expect("invalid defaults");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex invalid-default-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_EFFORT")
        .output()
        .expect("run invalid defaults fish launcher");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("source must be `explicit` or `settings`")
    );
}

#[test]
fn fish_launcher_uses_claude_settings_model_and_effort_when_available() {
    let home = tempfile::tempdir().expect("temporary settings launcher home");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::create_dir_all(home.path().join(".claude")).expect("settings directory");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::write(
        home.path().join(".claude/settings.json"),
        "{\"model\":\"sonnet[1m]\",\"effortLevel\":\"high\"}",
    )
    .expect("fixture settings");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1,\"mainProviders\":[\"p\"],\"providers\":[{\"id\":\"p\",\"agent\":\"worker\",\"defaultModel\":\"provider-model\",\"subagentModel\":\"worker-model\",\"effort\":\"high\",\"backend\":\"codex-app-server\"}],\"fallback\":{\"agent\":\"fallback\",\"model\":\"sonnet\",\"effort\":\"high\"}}",
    )
    .expect("fixture provider config");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=%s\\n' \"${CLAUDE_CODE_ALWAYS_ENABLE_EFFORT:-}\"\nprintf '%s\\n' \"$@\"\n",
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
            &format!("source '{}'; claudex settings-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run fish launcher with settings");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 settings launcher arguments");
    assert!(arguments.contains("--model\nprovider-model\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains(&format!(
        "--provider-config\n{}\n",
        home.path().join(".config/claudex/providers.json").display()
    )));
    assert!(arguments.ends_with(
        "--\n--agent\nclaudex-orchestrator\n--allowedTools\nWebSearch,WebFetch\nsettings-smoke\n"
    ));
    assert!(String::from_utf8_lossy(&output.stderr).contains("settings sonnet[1m], high"));
}

#[test]
fn fish_config_sets_the_plain_claude_subagent_limit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config =
        fs::read_to_string(root.join(".config/fish/config.fish")).expect("fish configuration");
    assert!(
        config
            .lines()
            .any(|line| line.contains("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"))
    );
    assert!(
        config
            .lines()
            .any(|line| line.contains("or set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS 40"))
    );
    assert!(
        config.lines().any(|line| {
            line.contains(
            "set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; and set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
        )
        }) || config.lines().any(|line| line.contains(
            "set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; or set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
        ))
    );
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
    assert!(
        arguments
            .ends_with("--\n--allowedTools\nWebSearch,WebFetch\n--agent\ncustom-subagent\nsmoke\n")
    );
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
fn subagent_routes_and_fallback_match_worker_definitions() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".config/claudex/providers.json")).expect("provider config"),
    )
    .expect("valid provider config");
    let configured = config["providers"]
        .as_array()
        .expect("configured providers")
        .iter()
        .map(|provider| {
            (
                &provider["agent"],
                provider
                    .get("subagentModel")
                    .unwrap_or(&provider["defaultModel"]),
                &provider["effort"],
            )
        })
        .chain(std::iter::once((
            &config["fallback"]["agent"],
            &config["fallback"]["model"],
            &config["fallback"]["effort"],
        )));
    for (agent, model, effort) in configured {
        let name = format!("{}.md", agent.as_str().expect("worker agent"));
        let model = model.as_str().expect("worker model");
        let effort = effort.as_str().expect("worker effort");
        let definition = fs::read_to_string(root.join(".claude/agents").join(&name))
            .expect("provider worker definition");
        for expected in [format!("model: {model}"), format!("effort: {effort}")] {
            assert!(
                definition.lines().any(|line| line == expected),
                "{name} must match the shared provider route: {expected}"
            );
        }
    }

    assert_qwen_runtime_is_bounded(&root, &config);
    assert_provider_children_skip_claude_session_hook(&root);
}

#[test]
fn every_non_advisor_subagent_inherits_the_main_tool_and_permission_context() {
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/agents");
    for entry in fs::read_dir(agents).expect("Claude agent definitions") {
        let path = entry.expect("agent directory entry").path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("md") {
            continue;
        }
        let file_name = path.file_name().and_then(std::ffi::OsStr::to_str);
        if file_name == Some("custom-advisor.md") {
            continue;
        }
        let definition = fs::read_to_string(&path).expect("Claude agent definition");
        let frontmatter = definition
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .collect::<Vec<_>>();
        if file_name == Some("claudex-haiku-search.md") {
            assert!(
                frontmatter.contains(&"tools: WebSearch,WebFetch"),
                "{file_name:?} must expose only live web retrieval tools"
            );
            assert!(
                definition.contains("Dedicated live-web retrieval worker"),
                "{file_name:?} must document its bounded retrieval scope"
            );
            continue;
        }
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

#[test]
fn custom_advisor_has_an_explicit_isolated_peer_messaging_channel() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.claude/agents/custom-advisor.md");
    let definition = fs::read_to_string(&path).expect("custom advisor definition");
    let frontmatter = definition
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .collect::<Vec<_>>();
    let tool_lines = frontmatter
        .iter()
        .filter(|line| line.starts_with("tools:"))
        .copied()
        .collect::<Vec<_>>();

    assert_eq!(tool_lines, ["tools: SendMessage"]);
    for forbidden_field in ["disallowedTools:", "permissionMode:"] {
        assert!(
            !frontmatter
                .iter()
                .any(|line| line.starts_with(forbidden_field)),
            "custom-advisor must declare only its SendMessage channel, not {forbidden_field}"
        );
    }
    assert!(
        definition.contains("deliberate exception to normal worker inheritance"),
        "custom-advisor must document why it does not inherit worker tools"
    );
    assert!(
        definition.contains("only tool is")
            && definition.contains("`SendMessage`")
            && definition.contains("peer-advisory channel"),
        "custom-advisor must document its isolated peer-messaging channel"
    );
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
    assert!(arguments.contains("--model\nmodel\n"));
    assert!(
        arguments
            .ends_with("--\n--agent\nclaudex-orchestrator\n--allowedTools\nWebSearch,WebFetch\n")
    );
}
