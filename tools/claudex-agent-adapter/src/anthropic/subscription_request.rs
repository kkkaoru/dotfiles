use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MessagesRequest, content::system_text};

pub(super) fn subscription_request_prompt(request: &MessagesRequest) -> String {
    format!(
        concat!(
            "Act as the requested Claude Code model. Follow the system instructions and complete ",
            "the conversation below. Use only the enabled tools when needed. Delegation is the ",
            "standing default for substantive work unless the user opts out. When selected_workers ",
            "are present, invoke the selected Agent or Task directly as the first tool call; do not ",
            "perform task-list bookkeeping first. Apply current selected_workers routing to every ",
            "Agent/Task launch, including nested launches from an existing worker: choose the ",
            "selected claudex worker agent and pass its exact model and effort. Never default a ",
            "nested launch to generic claude or blindly inherit its parent provider when current ",
            "routing selects another route. Start as many worker instances as useful for real ",
            "parallelism. Treat disabled_subagent_models in the current routing context as an ",
            "absolute SubAgent denylist, including explicit, inherited, nested, and reused routes. ",
            "When launching multiple independent workers, emit every intended ",
            "Agent/Task call together in the same assistant message and tool round; never emit one ",
            "call and defer the rest to later turns. Do not announce a worker count until that same ",
            "message contains exactly that many launch calls. When the main session needs their ",
            "results now, use foreground calls; use background only when useful independent work ",
            "can continue or the task must outlive the turn. Continuing work must be a concrete next ",
            "action already identified and started immediately. After successful background launches, ",
            "start that action or end the turn promptly with concise user-visible status; never keep ",
            "reasoning while waiting for completion notifications. For a related follow-up, use SendMessage ",
            "with the exact compatible recipient specified by the prior Agent/Task result (agent ID ",
            "or teammate name as applicable), sending the smallest sufficient self-contained delta ",
            "including unseen evidence. Do not send a mid-flight message merely to repeat scope or ",
            "restrictions already present in the original delegation. A follow-up queued to a busy ",
            "worker does not add parallel capacity; assign genuinely independent work to another ",
            "routed worker when useful capacity exists. Reuse the first compatible session advisor for related ",
            "decisions, including after completion; start another only for true parallel or ",
            "clean-room review, incompatible context, or an unavailable recipient. Before shutdown ",
            "or replacement, weigh likely follow-ups and potential prompt-prefix/cache reuse against ",
            "slot/resource pressure and stale context; neither creation nor termination is ",
            "categorically forbidden. Treat current routing context as authoritative over stale ",
            "model-policy memory. When a Task or Agent tool schema lacks claudex_model or ",
            "claudex_effort, put each routed value at the start of its prompt as an exact ",
            "`claudex_model: <model>` or `claudex_effort: <effort>` line. Never put an external ",
            "provider model ID in the native model field.\n\nSystem:\n{}\n\nMessages:\n{}"
        ),
        system_text(&request.system),
        serde_json::to_string(&request.messages).unwrap_or_default()
    )
}

pub(super) fn requested_tools(tools: &[Value], omit_task_bookkeeping: bool) -> Vec<String> {
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .filter(|name| {
            !omit_task_bookkeeping
                || !matches!(*name, "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet")
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
