use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .parent()
        .expect("repository root")
        .to_owned()
}

fn assert_command_capable_worker(root: &Path, agent: &str, model: &str, effort: &str) {
    let path = root.join(format!(".claude/agents/{agent}.md"));
    let definition = fs::read_to_string(&path).expect("worker definition");
    let frontmatter = definition
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .collect::<Vec<_>>();

    for expected in [
        format!("name: {agent}"),
        format!("model: {model}"),
        format!("effort: {effort}"),
    ] {
        assert!(
            frontmatter.iter().any(|line| *line == expected),
            "{} must match its configured route: {expected}",
            path.display()
        );
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
        definition.contains("complete tool set and permission context"),
        "{} must document inherited shell and command access",
        path.display()
    );
}

#[test]
fn every_model_worker_inherits_shell_and_command_capability() {
    let agents_dir = repository_root().join(".claude/agents");
    let mut agent_files = fs::read_dir(&agents_dir)
        .expect("agent definitions")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("claudex-") || name == "custom-advisor.md")
        })
        .collect::<Vec<_>>();
    agent_files.sort();
    assert!(
        !agent_files.is_empty(),
        "at least one model worker is required"
    );

    for path in agent_files {
        let content = fs::read_to_string(&path).expect("read agent definition");
        let name = path.display();
        assert!(
            !content.lines().any(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("tools:")
                    || trimmed.starts_with("disallowedTools:")
                    || trimmed.starts_with("permissionMode:")
            }),
            "{name} must not restrict its inherited tool set"
        );
        assert!(
            content.contains("complete tool set and permission context"),
            "{name} must document inherited shell and command access"
        );
        assert!(
            !content.contains("Do not perform filesystem, shell, MCP, Agent, or Task"),
            "{name} must not impose a shell restriction"
        );
    }
}

#[test]
fn opencode_provider_starts_the_acp_subcommand() {
    let root = repository_root();
    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".config/claudex/providers.json"))
            .expect("provider configuration"),
    )
    .expect("valid provider configuration");
    let provider = config["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|provider| provider["id"] == "opencode-go")
        .expect("OpenCode provider");
    let arguments = provider["acp"]["arguments"]
        .as_array()
        .expect("OpenCode ACP arguments");
    assert_eq!(
        arguments,
        &[Value::String("acp".to_owned())],
        "OpenCode ACP permissions are approved through the ACP client; TUI-only flags must not intercept the subcommand"
    );
}

#[test]
fn configured_worker_routes_are_command_capable() {
    let root = repository_root();
    let codex_config =
        fs::read_to_string(root.join("tools/claudex-agent-adapter/src/app_server.rs"))
            .expect("Codex app-server source");
    for feature in [
        "shell_tool = true",
        "unified_exec = true",
        "tool_search = true",
    ] {
        assert!(
            codex_config.contains(feature),
            "Codex app-server routes must enable {feature}"
        );
    }
    let grok_command =
        fs::read_to_string(root.join("tools/claudex-agent-adapter/src/grok_acp/connection.rs"))
            .expect("Grok ACP source");
    assert!(
        grok_command.contains("--always-approve"),
        "Grok ACP routes must allow command tools"
    );
    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".config/claudex/providers.json"))
            .expect("provider configuration"),
    )
    .expect("valid provider configuration");
    let providers = config["providers"].as_array().expect("providers array");

    for (id, backend) in [
        ("codex", "codex-app-server"),
        ("codex-spark", "codex-app-server"),
        ("fugu", "codex-app-server"),
        ("ollama-glm-5-2", "codex-app-server"),
        ("grok", "grok-acp"),
        ("opencode-go", "configured-acp"),
    ] {
        let provider = providers
            .iter()
            .find(|provider| provider["id"] == id)
            .unwrap_or_else(|| panic!("{id} provider"));
        assert_eq!(provider["backend"], backend, "{id} backend");

        let agent = provider["agent"].as_str().expect("provider worker agent");
        let model = provider
            .get("subagentModel")
            .unwrap_or(&provider["defaultModel"])
            .as_str()
            .expect("provider worker model");
        let effort = provider["effort"].as_str().expect("provider worker effort");
        assert_command_capable_worker(&root, agent, model, effort);
    }

    let native_workers = config["nativeWorkers"]
        .as_array()
        .expect("native worker routes");
    assert!(
        native_workers.iter().any(|worker| {
            worker["agent"] == "claudex-haiku-search" && worker["model"] == "claude-haiku-4-5"
        }),
        "native Haiku must remain a configured worker"
    );
    for worker in native_workers {
        assert_command_capable_worker(
            &root,
            worker["agent"].as_str().expect("native worker agent"),
            worker["model"].as_str().expect("native worker model"),
            worker["effort"].as_str().expect("native worker effort"),
        );
    }

    let fallback = &config["fallback"];
    assert_eq!(fallback["agent"], "claudex-sonnet");
    assert_eq!(fallback["model"], "claude-sonnet-5");
    assert_command_capable_worker(
        &root,
        fallback["agent"].as_str().expect("fallback worker agent"),
        fallback["model"].as_str().expect("fallback worker model"),
        fallback["effort"].as_str().expect("fallback worker effort"),
    );
}
