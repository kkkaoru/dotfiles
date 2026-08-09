use std::collections::HashMap;

use serde_json::{Value, json};

use super::super::MessagesRequest;
use crate::agent_backend::WebSearchMode;
mod thread;
#[cfg(test)]
pub(in crate::anthropic) use thread::thread_start_params;
pub(in crate::anthropic) use thread::thread_start_params_for_mode;

const ORCHESTRATOR_INSTRUCTIONS: &str = "Claudex main-session orchestration mode is active. The main session must control parallel distribution across multiple SubAgents for independent work: coordinate, decompose into non-redundant workstreams, choose fan-out for current capacity, delegate, monitor, resolve conflicts, synthesize worker results, and deliver the final response. Claude Code's enabled tools, permission rules, hooks, MCP servers, skills, and Agent Teams remain available in this session. For every substantive investigation, implementation, review, or validation, call a routed Agent/Task worker by default rather than doing the work in main. This remains mandatory after long execution, compaction, resume, context reconstruction, and worker failure. Avoid serial heavy processing by one worker when capacity allows multi-worker fan-out: unless the work is truly indivisible, the user opts out, or only one compatible worker slot is available, launch each ordinary worker as an independent native Agent/Task background call; multiple calls may be emitted in one assistant response, but never use an adapter-only batch wrapper or wait for every result before integrating completed work. Do not give an entire heavy or unknown-duration task to one ordinary worker merely for convenience. custom-advisor is a separate logical session singleton/capacity channel, not an implementation workstream, and built-in advisor remains independent of worker capacity. For complex or ambiguous decisions, external research or multiple sources, high-risk configuration, phases exceeding ten minutes, worker stalls/timeouts, or conflicting worker results, consult one custom-advisor when triggered unless a compatible advisor is already active; use its native result for related decisions instead of launching a replacement. Do not invoke custom-advisor for trivial work. For ordinary related follow-ups, reuse the exact prior recipient by setting resume on Agent/Task and continue with native Agent/Task results and TaskOutput rather than SendMessage; SendMessage is reserved for an explicitly active Agent Teams session. When a live user follow-up targets a running Command Code or other one-shot ACP worker that has no Claude tool round, do not leave the instruction queued for the next tool round: TaskStop the exact active Agent id immediately, then Agent/Task with resume set to that agentId (or a fresh launch if already stopped) carrying the new user instruction. If the agents panel shows queued on that worker, TaskStop immediately and resume or relaunch with the queued user text; cmd -p never has a Claude tool round that would flush the inbox. Command Code and other one-shot ACP workers run tools provider-side, so the parent Task card often shows tool_uses: 0; do not tell the user the worker did no search or tool work from that count. Human-visible sync is the agents panel (name, elapsed, tokens), SubAgent thinking/? chrome with ▶/✓ tool markers, and display-only web cards. Never copy end-the-turn-with-status or emit-short-status-after-each-phase into Agent/Task worker prompts; those rules apply only after you launch workers. Worker prompts must require tool-backed completion and concrete evidence. Treat a status-only toolless worker result as failure and reroute; do not accept it as done.";
/// Bridge text for ACP-native providers: they execute their own tools, not Claude Code tools.
const ACP_NATIVE_BRIDGE_INSTRUCTIONS: &str = "You are the model inside Claudex on a provider-native ACP backend (Cursor, OpenCode, or Grok). The provider owns filesystem, shell, and search tools—use them for implementation. Keep native thinking streaming for the whole turn; do not emit repeated status chrome or launch-metadata echoes. Only if your tool inventory includes Agent or Task (Claude Code SubAgent launch tools) may you launch Claudex workers by those exact names; the adapter bridges those to Claude Code. Do not invent Agent/Task invocations when those tools are absent. Preserve task-specific restrictions the active user requires, but do not invent read-only or no-edit limits.";
/// Instructions for ACP-native agent loops (Cursor / OpenCode / Grok). Claude Code Agent/Task
/// tools are not executable here; forcing them causes silent thrashing and no progress UI.
const ACP_NATIVE_ORCHESTRATOR_INSTRUCTIONS: &str = "Claudex provider-native ACP mode is active. Do the work with your provider-native tools (shell, read, edit, search, web as offered). Keep Claude Code native thinking streaming for the whole turn; do not emit repeated factual status chrome, launch-metadata echoes, or Thought-for placeholders. Keep the loop tight: inspect, act, validate; avoid repeated self-dialogue. Prefer batched independent inspections over serial one-file probes. For Claudex worker fan-out, launch through Agent or Task when available (bridged to Claude Code tool_use so the agents panel tracks status). Use one selected_workers entry's subagent_type, exact claudex_model, and exact claudex_effort as an inseparable tuple; never mix fields from different workers. Set run_in_background=true. If only spawn_subagent is available, use it—the adapter bridges it to Agent tool_use for Claudex tracking. After a background launch, post a brief status and end the turn promptly; never block the same turn with get_command_or_subagent_output/TaskOutput waits that use a large positive timeout. Pull results only after a Claude completion notification on a later turn, or with a non-blocking snapshot. Report blockers promptly and finish with a concise result, remaining risks, and files or commands involved. In the main session only, return the answer directly when no tool is needed. Never copy end-the-turn-with-status or emit-short-status-after-each-phase into delegated worker prompts.";
const ACP_NATIVE_WORKER_INSTRUCTIONS: &str = "You are a provider-native ACP worker. Complete the delegated task with your native tools. This is a new one-shot task: ignore prior chats, memory, and git dirty state; do not reconstruct history. Stream native thinking continuously; do not emit repeated status chrome or launch-metadata echoes. A short status or phase update is never a complete answer: if the parent prompt asks for status after each phase, emit it only between native tool work, never instead of finishing the task. Do not end the turn after a toolless status-only message. Prefer finishing work natively; if you nest further, prefer Agent/Task (or spawn_subagent, which is bridged) so Claudex can track nested workers. Do not same-turn wait on a long get_command_or_subagent_output timeout. Stay within the stated scope and return concrete evidence.";
const SUBAGENT_MAIN_ONLY_TOOLS_INSTRUCTIONS: &str = "Built-in Claude Code advisor() is main-session only and is not executable in this SubAgent. Do not call advisor() or invent that tool. Continue the delegated task with the tools you have. Do not launch models listed in disabled_subagent_models; if a nested worker is needed, use a selected_workers entry or custom-advisor via Agent/Task.";
const SUBAGENT_LIFECYCLE_INSTRUCTIONS: &str = "For independent fan-out that may be long-running or whose duration is unknown, set run_in_background=true on every Agent/Task launch unless the active user explicitly requires synchronous results. Keep each launch as an independent native call; multiple launches may be emitted in one assistant response, but never use an adapter-only batch wrapper and never wait for the slowest worker before accepting another user instruction. Do not mix foreground and background launches. Background completion notifications are integrated incrementally on later turns, so start a concrete independent action or end the current turn promptly instead of reasoning while waiting for the slowest worker. Do not automatically poll TaskList or TaskOutput on a timer; handle the next user message first and retrieve only the exact task output needed by that message or an unresolved dependency. Use foreground only for short bounded work, a dependency-required result, or an explicit synchronous request. This rule supersedes generic foreground advice above when a worker may be heavy. An explicit active user request for an exact worker count is a hard cardinality constraint: emit exactly that many independent Agent/Task launches, including exactly one for a one-worker request, never duplicate or retry the launch, and override every minimum-parallelism or fan-out default. For ordinary follow-ups, reuse the exact Agent/Task recipient through its native result and TaskOutput; set resume to that exact agentId instead of launching a replacement. Do not send progress or results through SendMessage. TaskStop/Stop Task is best-effort and idempotent: use only the exact active task_id returned by the current Agent/Task result, never guessed, previous-session, or notification-only IDs. Never stop a mailbox name, completion notification, previous-session orphan ID, or already-consumed task. If the tool reports `No task found`, treat it as already stopped/completed, do not retry, and continue without cascading stops onto unrelated in-flight workers. One scope/description key may have at most one in-flight worker; never relaunch the same key while a peer for that scope is still running, and never mass-stop healthy peers because one lane failed. Use SendMessage only when Agent Teams is explicitly active.";

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
    let allow_team_messages = super::super::subagent_reuse::agent_teams_enabled(request);
    let (tools, external_names) = external_tools(
        &request.tools,
        web_search_mode,
        super::super::subagent_reuse::should_expose_launch_tools(request),
        allow_team_messages,
        super::super::agent_effort::is_subagent_request(request),
    );
    // Provider-side tools must be an exact projection of the schemas supplied by
    // Claude Code.  Advisor/collaborator work is therefore exposed only when
    // Claude Code sends a public tool schema; the adapter never synthesizes or
    // executes an invisible inference tool of its own.
    (tools, external_names, HashMap::new())
}

pub(in crate::anthropic) fn is_main_session_only_tool(name: &str) -> bool {
    let stem = name
        .strip_prefix("cc_")
        .unwrap_or(name)
        .split(['_', '-'])
        .next()
        .unwrap_or(name);
    stem.eq_ignore_ascii_case("advisor")
}

fn external_tools(
    tools: &[Value],
    web_search_mode: WebSearchMode,
    expose_launch_tools: bool,
    allow_team_messages: bool,
    hide_main_only_tools: bool,
) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if hide_main_only_tools && is_main_session_only_tool(original_name) {
            continue;
        }
        if web_search_mode == WebSearchMode::CodexNative && original_name == "WebSearch" {
            continue;
        }
        // Keep the original index when projecting Claude Code's dynamic tools.
        // The provider may return a tool call using that index, so filtering a
        // cloned list before enumerate() would route TaskUpdate/TaskOutput to
        // the wrong tool.
        if !allow_team_messages && original_name == "SendMessage" {
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
        "Prefer reuse over replacement: continue compatible workers through native Agent/Task resume=<agentId>, then launch fresh workers only when reuse is not safe or not possible. SendMessage is restricted to explicit Agent Teams. For completed sub-tasks, revive the same-scope worker with Agent/Task resume instead of launching a replacement; launch fresh workers only when the prior worker failed, was cancelled/stopped, or the scope is independent."
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
        "Agent" | "Task" => {
            " When a compatible SubAgent already exists for this scope, set resume to that exact agentId instead of launching a replacement. Launch a new worker only for independent scope or a failed/cancelled/stopped prior worker."
        }
        "TaskStop" | "StopTask" | "Stop Task" => {
            " Task lifecycle: stopping is idempotent; use only the exact active Agent task_id from the current launch (`a` + 16 hex). Never guess IDs, never stop Bash-background nanoids (e.g. b13mjnjlj) or previous-session orphan IDs from `No completion record` notifications, and never cascade stops onto unrelated in-flight workers after one lane fails. A `No task found` response means already stopped/completed; do not retry."
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
