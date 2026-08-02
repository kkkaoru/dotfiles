use std::collections::HashMap;

use serde_json::{Value, json};

use super::super::MessagesRequest;
use crate::agent_backend::WebSearchMode;
mod thread;
#[cfg(test)]
pub(in crate::anthropic) use thread::thread_start_params;
pub(in crate::anthropic) use thread::thread_start_params_for_mode;

const ORCHESTRATOR_INSTRUCTIONS: &str = "Claudex main-session orchestration mode is active. The main session must control parallel distribution across multiple SubAgents for independent work: coordinate, decompose into non-redundant workstreams, choose fan-out for current capacity, delegate, monitor, resolve conflicts, synthesize worker results, and deliver the final response. Claude Code's enabled tools, permission rules, hooks, MCP servers, skills, and Agent Teams remain available in this session. For every substantive investigation, implementation, review, or validation, call a routed Agent/Task worker by default rather than doing the work in main. This remains mandatory after long execution, compaction, resume, context reconstruction, and worker failure. Avoid serial heavy processing by one worker when capacity allows multi-worker fan-out: unless the work is truly indivisible, the user opts out, or only one compatible worker slot is available, launch parallel ordinary workers in the same batch; do not give an entire heavy or unknown-duration task to one ordinary worker merely for convenience. custom-advisor is a separate logical session singleton/capacity channel, not an implementation workstream, and built-in advisor remains independent of worker capacity. For complex or ambiguous decisions, external research or multiple sources, high-risk configuration, phases exceeding ten minutes, worker stalls/timeouts, or conflicting worker results, consult one custom-advisor when triggered unless a compatible advisor is already active; reuse that advisor with SendMessage for related decisions instead of launching a replacement. Do not invoke custom-advisor for trivial work. For related follow-ups, reuse compatible workers with SendMessage and the exact prior Agent/Task recipient instead of churning processes with fresh launches; start a new instance only when true concurrency, clean-room review, a different route/role, incompatible scope, or an unavailable recipient requires it.";
const SUBAGENT_LIFECYCLE_INSTRUCTIONS: &str = "For independent fan-out that may be long-running or whose duration is unknown, set run_in_background=true on every launch in the single batch unless the active user explicitly requires synchronous results. Do not mix foreground and background launches in one batch. Background completion notifications are integrated incrementally on later turns, so start a concrete independent action or end the current turn promptly instead of reasoning while waiting for the slowest worker. Use foreground only for short bounded work, a dependency-required result, or an explicit synchronous request. This rule supersedes generic foreground advice above when a worker may be heavy. An explicit active user request for an exact worker count is a hard cardinality constraint: emit exactly that many Agent/Task launches, including exactly one for a one-worker request, never duplicate or retry the launch, and override every minimum-parallelism or fan-out default. Prefer reusing a compatible recipient via SendMessage over launching a replacement process solely to continue related work. TaskStop/Stop Task is best-effort and idempotent: use only the exact active task_id returned by the current Agent/Task result, never guessed or stale IDs. Never stop a mailbox name, completion notification, or already-consumed task. If the tool reports `No task found`, treat it as already stopped/completed, do not retry, and continue.";

#[cfg(test)]
pub(in crate::anthropic) fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    tool_configuration_for_mode(
        request,
        advisor_model,
        collaborator_model,
        WebSearchMode::default(),
    )
}

pub(in crate::anthropic) fn tool_configuration_for_mode(
    request: &MessagesRequest,
    _advisor_model: Option<&str>,
    _collaborator_model: Option<&str>,
    web_search_mode: WebSearchMode,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let provider_tools = request.tools.clone();
    let (tools, external_names) = external_tools(
        &provider_tools,
        web_search_mode,
        super::super::subagent_reuse::should_expose_launch_tools(request),
    );
    // Provider-side tools must be an exact projection of the schemas supplied by
    // Claude Code.  Advisor/collaborator work is therefore exposed only when
    // Claude Code sends a public tool schema; the adapter never synthesizes or
    // executes an invisible inference tool of its own.
    (tools, external_names, HashMap::new())
}

fn external_tools(
    tools: &[Value],
    web_search_mode: WebSearchMode,
    expose_launch_tools: bool,
) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if web_search_mode == WebSearchMode::CodexNative && original_name == "WebSearch" {
            continue;
        }
        if !expose_launch_tools && super::super::subagent_reuse::is_launch_tool(original_name) {
            tracing::warn!(
                tool = original_name,
                "hiding native SubAgent launch after session budget"
            );
            continue;
        }
        let codex_name = codex_tool_name(original_name, index);
        let spec = dynamic_tool(tool, &codex_name).expect("tool name was validated");
        names.insert(codex_name, original_name.to_owned());
        specs.push(spec);
    }
    (specs, names)
}

fn parallel_scheduler_instructions(request: &MessagesRequest) -> String {
    let scheduler = crate::parallel_scheduler::ParallelScheduler::shared();
    let config = scheduler.config();
    let cadence_minutes = (config.reassess_interval.as_secs() / 60).max(1);
    let mut lines = vec![
        format!(
            "Runtime parallel policy: choose one ordinary worker for one indivisible scope, two for two independent scopes, and fan out to at least {} ordinary workers across at least {} model families only when three or more scopes justify it; selected_workers is a capacity pool, not a launch count; maintain at least {} active lanes during a completion or cadence rebalance.",
            config.min_parallel_workers, config.min_model_families, config.active_floor
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
    let lifecycle_guidance = task_lifecycle_guidance(original_name);
    Some(json!({
        "type": "function",
        "name": codex_name,
        "description": format!(
            "Claude Code tool `{original_name}`. {}{}",
            tool.get("description").and_then(Value::as_str).unwrap_or(""),
            lifecycle_guidance
        ),
        "inputSchema": tool.get("input_schema").cloned()
            .unwrap_or_else(|| json!({"type":"object"}))
    }))
}

fn task_lifecycle_guidance(tool_name: &str) -> &'static str {
    match tool_name {
        "TaskStop" | "StopTask" | "Stop Task" => {
            " Task lifecycle: stopping is idempotent; use only the exact active task_id returned by the current Agent/Task launch. Never guess IDs or stop completed/notification-consumed tasks. A `No task found` response means already stopped/completed; do not retry."
        }
        _ => "",
    }
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
