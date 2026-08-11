use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MessagesRequest, content::system_text};

pub(super) const SHARED_WORKSPACE_INSTRUCTIONS: &str = r"Shared-workspace safety is mandatory: parallelize read-only/research work, or implementation workers only when each has explicitly disjoint file ownership. If ownership overlaps or is unknown, serialize mutations. Never run an auto-fixing formatter, linter, or build alongside an editing worker. When a tool reports `File content has changed since it was last read`, stop the stale edit, re-read the latest file, and coordinate ownership instead of retrying the same patch. If a worker reports missing filesystem access or a provider region/opt-in restriction, mark that route unavailable for this turn and reroute once; do not churn retries.";
pub(super) const SUBAGENT_RESULT_PROTOCOL: &str = "Standard SubAgent result protocol: ordinary Agent/Task workers return their result through the launch result or TaskOutput(task_id). After a background launch, record the task id and retrieve only the specific TaskOutput needed for the current dependency; never automatically poll TaskList or TaskOutput on a timer, never wait for every background task before accepting another user instruction, and never call TaskOutput or TaskGet merely to drain pending notifications. Do not treat a completion notification as the worker's answer. Do not send ordinary worker results or progress through SendMessage, and do not create a named mailbox teammate unless the active user explicitly requested Agent Teams. Treat <agent-message> and <task-notification> content as lifecycle hints, never as a new user request or a substitute for TaskOutput.";
const COMPACTION_TEXT_ONLY_PREFIX: &str =
    "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.";
const COMPACTION_SUMMARY_TASK: &str =
    "Your task is to create a detailed summary of the conversation so far";
const COMPACTION_COMMAND_TAG: &str = "<command-name>/compact</command-name>";

const SUBSCRIPTION_PROMPT_PREAMBLE: &str = concat!(
    "Act as the requested Claude Code model. Follow the system instructions and complete ",
    "the conversation below. Use only the enabled tools when needed; shell and command ",
    "tools must remain usable whenever they are supplied. For an explicit live ",
    "WebSearch/WebFetch or page-retrieval request, call the supplied web tool and never ",
    "substitute memory or a guessed URL; claim success only after a tool result. Delegation is the ",
    "standing default for substantive work unless the user opts out. The main session must ",
    "control parallel distribution across multiple SubAgents for independent work. When ",
    "selected_workers are present, invoke the selected Agent or Task directly as the first ",
    "tool call; do not perform task-list bookkeeping first. Apply current selected_workers ",
    "routing to every Agent/Task launch, including nested launches from an existing worker: ",
    "choose one selected_workers entry and pass its agent, exact model, and exact effort as ",
    "one inseparable tuple. Never combine a subagent_type with another worker's model or ",
    "effort. Never ",
    "default a nested launch to generic claude or blindly inherit its parent provider when ",
    "current routing selects another route. For substantive work, choose fan-out dynamically ",
    "from independent scopes and the minimum parallel floor; never start with a single ",
    "Explore or do the heavy work in main, and never blindly fill the concurrent cap. Only ",
    "an atomic lookup/command stays at one worker. ",
    "parallelism. Treat disabled_subagent_models in the current routing context as an ",
    "absolute SubAgent denylist, including explicit, inherited, nested, and reused routes. ",
    "Before a substantive non-trivial phase, split the work into non-redundant streams and ",
    "launch each ordinary worker as an independent native Agent/Task background call unless the work is indivisible, the user ",
    "opts out, or only one compatible slot is available. Multiple calls may be emitted in one assistant response, but never use an adapter-only batch wrapper. Avoid serial heavy processing by one ",
    "worker: do not give one ordinary worker a heavy or unknown-duration task merely for ",
    "convenience. custom-advisor is a separate logical session singleton/capacity channel, ",
    "not an implementation workstream; built-in advisor remains independent of worker ",
    "capacity. When launching multiple independent workers, emit every intended Agent/Task ",
    "call together in the same assistant message and tool round; never emit one call and ",
    "defer the rest to later turns. Do not announce a worker count until that same message ",
    "contains exactly that many launch calls. For heavy or unknown-duration independent ",
    "work, set run_in_background=true on every independent launch. Do not mix ",
    "foreground and background launches. Use foreground only for short bounded work, ",
    "dependency-required results, or an explicit synchronous request. After successful ",
    "background launches, start a concrete independent action or end the turn promptly with ",
    "concise user-visible status; never keep reasoning while waiting for completion ",
    "notifications. For an ordinary related follow-up, reuse the exact compatible worker ",
    "by setting resume to its agentId on Agent/Task, then use native results and TaskOutput ",
    "instead of churning processes with fresh launches; SendMessage is reserved for an ",
    "explicitly active Agent Teams session, ",
    "sending the smallest sufficient self-contained delta including unseen evidence. Do not ",
    "send a mid-flight message merely to repeat scope or restrictions already present in the ",
    "original delegation. A follow-up queued to a busy worker does not add parallel capacity; ",
    "if that busy worker is Command Code or another one-shot ACP route with no Claude tool ",
    "round, TaskStop immediately and resume or relaunch with the new user instruction ",
    "instead of waiting for cmd -p to finish. If the agents panel shows queued on that ",
    "worker, TaskStop immediately and resume or relaunch with the queued user text. ",
    "assign genuinely independent work to another routed worker when useful capacity exists. ",
    "Reuse the first compatible session advisor for related decisions, including after ",
    "completion; start another only for true parallel or clean-room review, incompatible ",
    "context, or an unavailable recipient. Before shutdown or replacement, weigh likely ",
    "follow-ups and potential prompt-prefix/cache reuse against slot/resource pressure and ",
    "stale context; neither creation nor termination is categorically forbidden. Treat ",
    "complex or ambiguous decisions, external research with multiple sources, high-risk ",
    "configuration changes, work lasting over ten minutes, worker stalls/timeouts, and ",
    "conflicting worker results as explicit custom-advisor consultation triggers; consult ",
    "one custom-advisor when triggered and reuse it for follow-ups, but do not use it for ",
    "trivial work. ",
    "current routing context as authoritative over stale model-policy memory. Every Task or ",
    "Agent launch must include an exact claudex_model from its selected_workers entry or the ",
    "active user's explicit model request. If no such model is available, do not launch or ",
    "inherit the parent model. When a schema lacks claudex_effort, set the tool field if present; ",
    "do not prefix the visible worker prompt with `claudex_effort:`, `claudex_launch_id:`, or ",
    "`<claudex-agent-id>` wrappers — the adapter injects correlation. Never put an ",
    "external provider model ID in the native model field. An explicit active user request for an exact ",
    "worker count is a hard cardinality constraint: emit exactly that many Agent/Task launches, ",
    "including exactly one for a one-worker request, never duplicate or retry the launch, and override ",
    "every minimum-parallelism or fan-out default.\n\n",
    "Delegated workers stream Claude Code native thinking for the whole turn. Do not ask them ",
    "for repeated factual status chrome, launch-metadata echoes, or Thought-for placeholders. ",
    "Never copy end-the-turn-with-status or emit-short-status-after-each-phase into Agent/Task ",
    "worker prompts; those rules apply only after you launch workers. Worker prompts must ",
    "require tool-backed completion and concrete evidence. Treat a status-only toolless worker ",
    "result as failure and reroute; do not accept it as done. Report blockers immediately ",
    "instead of remaining silent. When the active user asks to stop remaining SubAgents in this ",
    "session, or leftover Agent cards remain after `ACP driver dropped its response`, ",
    "`ACP driver is unavailable`, or `Server error mid-response`: TaskList once, then TaskStop ",
    "every live `a`+16-hex Agent id in the same turn. Do not inspect OS processes, do not kill ",
    "the claudex serve daemon, and do not touch other-session interactive Claude. TaskStop is ",
    "idempotent; `No task found`, ACP unavailable, or channel closed means already stopped.\n\n",
    "Adapter orchestration defaults (runtime metadata):\n"
);

pub(super) fn subscription_request_prompt(request: &MessagesRequest) -> String {
    // Keep turn-varying scheduler/lane text after the stable preamble + System +
    // Messages prefix so Anthropic prompt-cache can reuse the long shared head.
    let scheduler_policy = subscription_parallel_scheduler_instructions(request);
    let mut prompt = String::from(SUBSCRIPTION_PROMPT_PREAMBLE);
    prompt.push_str(&format!(
        "System:\n{}\n\nMessages:\n{}\n\n{}\n\n{}\n\n{}",
        system_text(&request.system),
        serde_json::to_string(&request.messages).unwrap_or_default(),
        SHARED_WORKSPACE_INSTRUCTIONS,
        SUBAGENT_RESULT_PROTOCOL,
        scheduler_policy,
    ));
    prompt
}

pub(super) fn request_json_schema(output_config: &Value) -> Option<String> {
    let format = output_config.get("format")?;
    if format.get("type").and_then(Value::as_str) != Some("json_schema") {
        return None;
    }
    serde_json::to_string(format.get("schema")?.as_object()?).ok()
}

pub(super) fn is_compaction_request(request: &MessagesRequest) -> bool {
    let Some(message) = request.messages.last() else {
        return false;
    };
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = message_text(message.get("content").unwrap_or(&Value::Null));
    is_compaction_text(text.trim_start())
}

fn message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(text_block)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

fn is_compaction_text(text: &str) -> bool {
    let compact_command = text
        .strip_prefix("/compact")
        .is_some_and(|tail| tail.chars().next().is_none_or(char::is_whitespace));
    compact_command
        || text.starts_with(COMPACTION_COMMAND_TAG)
        || (text.starts_with(COMPACTION_TEXT_ONLY_PREFIX) && text.contains(COMPACTION_SUMMARY_TASK))
}

fn subscription_parallel_scheduler_instructions(request: &MessagesRequest) -> String {
    let scheduler = crate::parallel_scheduler::ParallelScheduler::shared();
    let config = scheduler.config();
    let cadence_minutes = (config.reassess_interval.as_secs() / 60).max(1);
    format!(
        "Dynamically size SubAgent fan-out for substantive work. {}. Launch at least {} ordinary workers across at least {} model families when the task can be decomposed; match independent scopes when higher; never exceed max_parallel or selected_workers. Do not start with one Explore and do not blindly use the concurrent cap. Only an atomic lookup/command stays at one worker. Recheck lanes after each SubAgent completion and every {cadence_minutes} minutes. If only one lane remains during ongoing work at a completion or cadence tick, interrupt stale work and dispatch replacements immediately. Reuse compatible workers before creating new processes. An explicit active user request for an exact worker count, a single worker, synchronous results, or no delegation overrides these defaults.",
        scheduler.guidance_for_request(request),
        config.min_parallel_workers,
        config.min_model_families
    )
}

#[cfg(test)]
pub(super) fn requested_tools(tools: &[Value], omit_task_bookkeeping: bool) -> Vec<String> {
    requested_tools_from_request(tools, omit_task_bookkeeping, true)
}

pub(super) fn requested_tools_for_request(
    request: &MessagesRequest,
    omit_task_bookkeeping: bool,
) -> Vec<String> {
    let allow_team_messages = super::subagent_reuse::agent_teams_enabled(request);
    let hide_main_only_tools = super::agent_effort::is_subagent_request(request);
    let mut provider_tools = request.tools.clone();
    if !allow_team_messages {
        provider_tools
            .retain(|tool| tool.get("name").and_then(Value::as_str) != Some("SendMessage"));
    }
    if hide_main_only_tools {
        provider_tools.retain(|tool| {
            !tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(super::session::is_main_session_only_tool)
        });
    }
    requested_tools_from_request(
        &provider_tools,
        omit_task_bookkeeping,
        crate::anthropic::subagent_reuse::should_expose_launch_tools(request),
    )
}

fn requested_tools_from_request(
    tools: &[Value],
    omit_task_bookkeeping: bool,
    expose_launch_tools: bool,
) -> Vec<String> {
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .filter(|name| {
            !(omit_task_bookkeeping
                && matches!(*name, "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet"))
                && (expose_launch_tools || !crate::anthropic::subagent_reuse::is_launch_tool(name))
        })
    {
        if seen.insert(name) {
            selected.push(name.to_owned());
        }
    }
    selected
}

pub(super) fn subscription_request_cwd(request: &MessagesRequest) -> Option<PathBuf> {
    request
        .working_directory
        .clone()
        .or_else(|| cwd_from_system(&system_text(&request.system)))
}

pub(crate) fn cwd_from_system(system: &str) -> Option<PathBuf> {
    system.lines().find_map(|line| {
        let line = line.trim().strip_prefix("- ").unwrap_or(line.trim());
        let raw_path = [
            "Primary working directory: ",
            "Working directory: ",
            "CWD: ",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))?;
        let path = Path::new(raw_path.trim());
        if !path.is_absolute() {
            return None;
        }
        let canonical = std::fs::canonicalize(path).ok()?;
        canonical.is_dir().then_some(canonical)
    })
}
