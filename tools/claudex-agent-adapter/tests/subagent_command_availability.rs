#[allow(dead_code)]
mod support;

use std::{
    fs,
    path::{Path, PathBuf},
};

use reqwest::Client;
use serde_json::Value;
use serde_json::json;
use support::{Adapter, post_json};

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
fn opencode_provider_maps_to_pi_gateway() {
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
    assert_eq!(provider["backend"], "pi-gateway");
    assert_eq!(provider["piProvider"], "opencode-go");
    assert_eq!(provider["piModel"], "deepseek-v4-pro");
    assert_eq!(provider.get("acp"), None);
}

#[test]
fn configured_worker_routes_are_command_capable() {
    let root = repository_root();
    let codex_config =
        fs::read_to_string(root.join("tools/claudex-agent-adapter/src/app_server/spawn.rs"))
            .expect("Codex app-server spawn source");
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
    let grok_command = fs::read_to_string(
        root.join("tools/claudex-agent-adapter/src/grok_acp/connection_command.rs"),
    )
    .expect("Grok ACP command source");
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
        ("codex", "pi-gateway"),
        ("codex-spark", "pi-gateway"),
        ("fugu", "pi-gateway"),
        ("ollama-glm-5-2", "pi-gateway"),
        ("grok", "pi-gateway"),
        ("opencode-go", "pi-gateway"),
        ("cursor", "pi-gateway"),
        ("cursor-luna", "pi-gateway"),
        ("cursor-sol", "pi-gateway"),
        ("cursor-terra", "pi-gateway"),
        ("command-code-luna", "pi-gateway"),
        ("devin", "pi-gateway"),
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

    for (id, agent, model, pi_model) in [
        (
            "cursor-luna",
            "claudex-cursor-luna",
            "cursor/gpt-5.6-luna",
            "gpt-5.6-luna",
        ),
        (
            "cursor-sol",
            "claudex-cursor-sol",
            "cursor/gpt-5.6-sol",
            "gpt-5.6-sol",
        ),
        (
            "cursor-terra",
            "claudex-cursor-terra",
            "cursor/gpt-5.6-terra",
            "gpt-5.6-terra",
        ),
    ] {
        let provider = providers
            .iter()
            .find(|provider| provider["id"] == id)
            .unwrap_or_else(|| panic!("{id} provider"));
        assert_eq!(provider["agent"], agent);
        assert_eq!(provider["defaultModel"], model);
        assert_eq!(provider["subagentModel"], model);
        assert_eq!(provider["modelPrefixes"], json!([model]));
        assert_eq!(provider["selectableModels"], json!([model]));
        assert_eq!(provider["piProvider"], "cursor");
        assert_eq!(provider["piModel"], pi_model);
        assert_eq!(provider.get("acp"), None);
        assert_eq!(provider["backend"], "pi-gateway");
        assert!(
            config["mainProviders"]
                .as_array()
                .is_some_and(|main| main.iter().any(|main_id| main_id == id)),
            "{id} must be a main-provider candidate"
        );
    }

    let command_code_luna_provider = providers
        .iter()
        .find(|provider| provider["id"] == "command-code-luna")
        .expect("Command Code Luna provider");
    assert_eq!(
        command_code_luna_provider["agent"],
        "claudex-command-code-luna"
    );
    assert_eq!(
        command_code_luna_provider["defaultModel"],
        "commandcode/gpt-5.6-luna"
    );
    assert_eq!(
        command_code_luna_provider["subagentModel"],
        "commandcode/gpt-5.6-luna"
    );
    assert_eq!(
        command_code_luna_provider["modelPrefixes"],
        json!(["commandcode/gpt-5.6-luna"])
    );
    assert_eq!(command_code_luna_provider["piProvider"], "commandcode");
    assert_eq!(command_code_luna_provider["piModel"], "gpt-5.6-luna");
    assert_eq!(command_code_luna_provider["effort"], "max");
    assert_eq!(command_code_luna_provider["maxContextTokens"], 800000);
    assert_eq!(command_code_luna_provider.get("acp"), None);
    assert_eq!(command_code_luna_provider["backend"], "pi-gateway");

    let devin_provider = providers
        .iter()
        .find(|provider| provider["id"] == "devin")
        .expect("Devin provider");
    assert_eq!(devin_provider["defaultModel"], "devin/swe-1-7");
    assert_eq!(devin_provider["subagentModel"], "devin/swe-1-7");
    assert_eq!(devin_provider["modelPrefixes"], json!(["devin/swe-1-7"]));
    assert_eq!(devin_provider["piProvider"], "devin");
    assert_eq!(devin_provider["piModel"], "swe-1-7");
    assert_eq!(
        devin_provider["piExtensions"],
        json!(["../../tools/pi-my-devin-cli-provider/index.ts"])
    );
    assert_eq!(devin_provider.get("acp"), None);
    assert_eq!(devin_provider["backend"], "pi-gateway");

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

#[tokio::test]
async fn codex_subagent_child_exposes_bash_and_accepts_a_harmless_git_gh_result() {
    let adapter = Adapter::start().await;
    let client = Client::new();
    let url = format!("{}/v1/messages", adapter.base_url);
    let system = "cc_is_subagent=true\n<claudex-agent-id>toolu_command_probe</claudex-agent-id>";
    let initial = post_json(
        &client,
        &url,
        json!({
            "model":"test-main-model", "system":system,
            "tools":[bash_tool()],
            "messages":[{"role":"user","content":"USE_COMMAND_TOOL"}]
        }),
    )
    .await;
    let tool = initial["content"]
        .as_array()
        .and_then(|content| content.first())
        .expect("Codex child must request the supplied Bash tool");
    assert_eq!(tool["type"], "tool_use");
    assert_eq!(tool["name"], "Bash");
    assert_eq!(
        tool["input"]["command"],
        "command -v git >/dev/null && command -v gh >/dev/null && printf CLAUDEX_COMMAND_PROBE_OK"
    );

    let completed = post_json(
        &client,
        &url,
        json!({
            "model":"test-main-model", "system":system,
            "messages":[
                {"role":"user","content":"USE_COMMAND_TOOL"},
                {"role":"assistant","content":initial["content"]},
                {"role":"user","content":[{
                    "type":"tool_result", "tool_use_id":tool["id"],
                    "content":"CLAUDEX_COMMAND_PROBE_OK"
                }]}
            ]
        }),
    )
    .await;
    assert_eq!(completed["content"][0]["text"], "CLAUDEX_COMMAND_PROBE_OK");
}

fn bash_tool() -> Value {
    json!({
        "name":"Bash", "description":"run shell commands",
        "input_schema":{"type":"object","properties":{"command":{"type":"string"}}}
    })
}
