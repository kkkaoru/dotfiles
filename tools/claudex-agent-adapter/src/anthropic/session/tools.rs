use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{Value, json};

use super::super::{BRIDGE_INSTRUCTIONS, MessagesRequest, content::system_text};
use crate::anthropic::subscription_request::cwd_from_system;

const ORCHESTRATOR_INSTRUCTIONS: &str = "Claudex main-session orchestration mode is active. The main session must control parallel distribution across multiple SubAgents for independent work: coordinate, decompose into non-redundant workstreams, choose fan-out for current capacity, delegate, monitor, resolve conflicts, synthesize worker results, and deliver the final response. Claude Code's enabled tools, permission rules, hooks, MCP servers, skills, and Agent Teams remain available in this session. For every substantive investigation, implementation, review, or validation, call a routed Agent/Task worker by default rather than doing the work in main. This remains mandatory after long execution, compaction, resume, context reconstruction, and worker failure. Avoid serial heavy processing by one worker when capacity allows multi-worker fan-out: unless the work is truly indivisible, the user opts out, or only one compatible worker slot is available, launch parallel ordinary workers in the same batch; do not give an entire heavy or unknown-duration task to one ordinary worker merely for convenience. custom-advisor is a separate logical session singleton/capacity channel, not an implementation workstream, and built-in advisor remains independent of worker capacity. For related follow-ups, reuse compatible workers with SendMessage and the exact prior Agent/Task recipient instead of churning processes with fresh launches; start a new instance only when true concurrency, clean-room review, a different route/role, incompatible scope, or an unavailable recipient requires it.";
const SUBAGENT_LIFECYCLE_INSTRUCTIONS: &str = "For independent fan-out that may be long-running or whose duration is unknown, set run_in_background=true on every launch in the single batch unless the active user explicitly requires synchronous results. Do not mix foreground and background launches in one batch. Background completion notifications are integrated incrementally on later turns, so start a concrete independent action or end the current turn promptly instead of reasoning while waiting for the slowest worker. Use foreground only for short bounded work, a dependency-required result, or an explicit synchronous request. This rule supersedes generic foreground advice above when a worker may be heavy. Prefer reusing a compatible recipient via SendMessage over launching a replacement process solely to continue related work.";

pub(in crate::anthropic) fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let selected_agents = selected_agents(request);
    let (mut tools, external_names) = external_tools(&request.tools, &selected_agents);
    let mut internal = HashMap::new();
    if let Some(model) = advisor_model {
        internal.insert("advisor".to_owned(), model.to_owned());
        tools.push(internal_advisor_tool());
    }
    let has_collaborator = request
        .tools
        .iter()
        .any(|tool| tool["name"] == "claude_collaborator");
    if let Some(model) = collaborator_model.filter(|_| !has_collaborator) {
        internal.insert("claude_collaborator".to_owned(), model.to_owned());
        tools.push(internal_collaborator_tool());
    }
    (tools, external_names, internal)
}

fn external_tools(
    tools: &[Value],
    selected_agents: &[String],
) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let mut routed_tool = tool.clone();
        if super::super::agent_batch::supports(original_name) {
            constrain_agent_types(&mut routed_tool, selected_agents);
        }
        let codex_name = codex_tool_name(original_name, index);
        let spec = dynamic_tool(&routed_tool, &codex_name).expect("tool name was validated");
        names.insert(codex_name, original_name.to_owned());
        specs.push(spec);
        if super::super::agent_batch::supports(original_name) {
            let batch_name = codex_tool_name(&format!("{original_name}_batch"), index);
            let spec = super::super::agent_batch::dynamic_tool(&routed_tool, &batch_name)
                .expect("agent tool name was validated");
            names.insert(
                batch_name,
                super::super::agent_batch::mapped_name(original_name),
            );
            specs.push(spec);
        }
    }
    (specs, names)
}

fn constrain_agent_types(tool: &mut Value, selected_agents: &[String]) {
    if selected_agents.is_empty() {
        return;
    }
    let Some(property) = tool
        .pointer_mut("/input_schema/properties/subagent_type")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let mut agent_types = property
        .get("enum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for selected_agent in selected_agents {
        if !agent_types.iter().any(|agent| agent == selected_agent) {
            agent_types.push(selected_agent.clone());
        }
    }
    property.insert("enum".to_owned(), json!(agent_types));
    property.insert(
        "description".to_owned(),
        Value::String(format!(
            "For routed Claudex workers, choose one of: {}. Claude Code's standard SubAgent types remain available when supplied by its schema.",
            selected_agents.join(", ")
        )),
    );
}

fn selected_agents(request: &MessagesRequest) -> Vec<String> {
    let Some(summary) = routing_texts(&request.system)
        .chain(request.messages.iter().flat_map(routing_texts))
        .filter_map(routing_summary)
        .last()
    else {
        return Vec::new();
    };
    let mut agents = summary
        .get("selected_agents")
        .cloned()
        .and_then(|agents| serde_json::from_value(agents).ok())
        .unwrap_or_default();
    add_explicit_provider_agents(request, &summary, &mut agents);
    agents
}

fn add_explicit_provider_agents(
    request: &MessagesRequest,
    summary: &Value,
    agents: &mut Vec<String>,
) {
    let requested = current_user_model_ids(request);
    if requested.is_empty() {
        return;
    }
    let mut denied = request
        .disabled_subagent_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    denied.extend(
        summary
            .get("disabled_subagent_models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    let Some(providers) = summary.get("providers").and_then(Value::as_object) else {
        return;
    };
    for provider in providers.values() {
        let Some(agent) = explicit_provider_agent(provider, &requested, &denied) else {
            continue;
        };
        if !agents.iter().any(|selected| selected == agent) {
            agents.push(agent.to_owned());
        }
    }
}

fn explicit_provider_agent<'a>(
    provider: &'a Value,
    requested: &[String],
    denied: &HashSet<&str>,
) -> Option<&'a str> {
    if provider.get("disabled").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let model = provider.get("model").and_then(Value::as_str)?;
    let prefixes = provider
        .get("model_prefixes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|prefix| !prefix.is_empty())
        .collect::<Vec<_>>();
    requested
        .iter()
        .any(|requested| {
            !denied.contains(requested.as_str())
                && (requested == model
                    || prefixes
                        .iter()
                        .any(|prefix| requested.starts_with(*prefix) && requested != prefix))
        })
        .then(|| provider.get("agent").and_then(Value::as_str))
        .flatten()
}

fn current_user_model_ids(request: &MessagesRequest) -> Vec<String> {
    let Some(content) = request
        .messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
    else {
        return Vec::new();
    };
    routing_texts(content)
        .flat_map(|text| {
            let explicit_text = text
                .split_once("{\"providers\":")
                .map_or(text, |(before_routing_context, _)| before_routing_context);
            explicit_text
                .split(|character: char| !is_model_id_character(character))
                .map(|token| {
                    token.trim_matches(|character: char| !character.is_ascii_alphanumeric())
                })
                .filter(|token| !token.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn is_model_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '-' | '_' | '.' | '/' | ':' | '@' | '+')
}

fn routing_summary(text: &str) -> Option<Value> {
    let start = text.find("{\"providers\":")?;
    Value::deserialize(&mut serde_json::Deserializer::from_str(&text[start..])).ok()
}

fn routing_texts(value: &Value) -> Box<dyn Iterator<Item = &str> + '_> {
    match value {
        Value::String(text) => Box::new(std::iter::once(text.as_str())),
        Value::Array(items) => Box::new(items.iter().flat_map(routing_texts)),
        Value::Object(object) => Box::new(object.values().flat_map(routing_texts)),
        _ => Box::new(std::iter::empty()),
    }
}

pub(in crate::anthropic) fn thread_start_params(
    request: &MessagesRequest,
    model: &str,
    dynamic_tools: Vec<Value>,
) -> Value {
    let system = system_text(&request.system);
    let cwd = request
        .working_directory
        .clone()
        .or_else(|| cwd_from_system(&system))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(isolated_runtime_cwd);
    let mut developer_instructions = super::super::team_protocol::guidance(&request.tools)
        .map_or_else(
            || BRIDGE_INSTRUCTIONS.to_owned(),
            |guidance| format!("{BRIDGE_INSTRUCTIONS}\n\n{guidance}"),
        );
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(&parallel_scheduler_instructions(request));
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(SUBAGENT_LIFECYCLE_INSTRUCTIONS);
    if !super::super::agent_effort::is_subagent_request(request) {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(ORCHESTRATOR_INSTRUCTIONS);
    }
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
    json!({
        "model": model,
        "cwd": cwd,
        "baseInstructions": base_instructions,
        "developerInstructions": developer_instructions,
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        // Codex built-in execution tools remain disabled below. Using workspace-write here
        // prevents the provider from misrepresenting Claude Code's dynamic tools as read-only.
        "sandbox": "workspace-write",
        "personality": "none",
        "config": {
            "web_search": "disabled",
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": false,
                "tool_search": false, "unified_exec": false, "web_search": false
            }
        }
    })
}

fn parallel_scheduler_instructions(request: &MessagesRequest) -> String {
    let scheduler = crate::parallel_scheduler::ParallelScheduler::shared();
    let config = scheduler.config();
    let cadence_minutes = (config.reassess_interval.as_secs() / 60).max(1);
    let mut lines = vec![
        format!(
            "Runtime parallel floor: launch at least {} ordinary workers when splitting substantive work; maintain at least {} active lanes while running. Use at least {} model families before work is considered sufficiently distributed.",
            config.min_parallel_workers, config.active_floor, config.min_model_families
        ),
        format!(
            "After each SubAgent completion and every {cadence_minutes} minutes, reassess active lanes. If only one active lane remains during ongoing work, interrupt stale work, redirect stale branches, and immediately add or replace workers to restore the floor."
        ),
        "Prefer reuse over replacement: continue compatible workers with SendMessage and send minimal follow-up context, then launch fresh workers only when reuse is not safe or not possible. For completed sub-tasks, replay same-scope work on fresh workers while expanding context on survivors when needed."
            .to_string(),
    ];
    lines.push(scheduler.guidance_for_request(request));
    lines.join("\n")
}

pub(in crate::anthropic) fn dynamic_tool(tool: &Value, codex_name: &str) -> Option<Value> {
    let original_name = tool.get("name")?.as_str()?;
    Some(json!({
        "type": "function",
        "name": codex_name,
        "description": format!(
            "Claude Code tool `{original_name}`. {}",
            tool.get("description").and_then(Value::as_str).unwrap_or("")
        ),
        "inputSchema": super::super::agent_effort::tool_schema(original_name,
            tool.get("input_schema").cloned()
                .unwrap_or_else(|| json!({"type":"object"})))
    }))
}

pub(in crate::anthropic) fn codex_tool_name(original_name: &str, index: usize) -> String {
    let sanitized = original_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let suffix = format!("_{index}");
    let maximum_name_bytes = 128usize.saturating_sub(3 + suffix.len());
    let stem = &sanitized[..sanitized.len().min(maximum_name_bytes)];
    format!("cc_{stem}{suffix}")
}

fn isolated_runtime_cwd() -> String {
    let home = match std::env::var_os("HOME") {
        Some(home) => std::path::PathBuf::from(home),
        None => std::path::PathBuf::from("/tmp"),
    };
    home.join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}

pub(in crate::anthropic) fn internal_advisor_tool() -> Value {
    json!({
        "type":"function",
        "name":"advisor",
        "description":"Ask the advisor model configured by Claude Code to independently review the entire conversation and return high-value guidance. It takes no parameters.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
    })
}

pub(in crate::anthropic) fn internal_collaborator_tool() -> Value {
    json!({
        "type":"function",
        "name":"claude_collaborator",
        "description":"Delegate an independent task to the collaborator model configured by Claude Code through the user's Claude subscription. Multiple calls may be issued in parallel.",
        "inputSchema":{
            "type":"object",
            "properties":{"task":{"type":"string","description":"The task for the Claude collaborator."}},
            "required":["task"],
            "additionalProperties":false
        }
    })
}
