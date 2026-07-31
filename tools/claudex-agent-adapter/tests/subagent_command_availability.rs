use std::{fs, path::PathBuf};

use serde_json::Value;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .parent()
        .expect("repository root")
        .to_owned()
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
fn opencode_provider_starts_in_auto_permission_mode() {
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
    assert!(
        arguments.iter().any(|argument| argument == "--auto"),
        "OpenCode ACP must auto-approve shell and command tools"
    );
}
