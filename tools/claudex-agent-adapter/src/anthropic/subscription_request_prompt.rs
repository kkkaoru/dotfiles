use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    MessagesRequest, SHARED_WORKSPACE_INSTRUCTIONS, SUBAGENT_RESULT_PROTOCOL,
    SUBSCRIPTION_PROMPT_PREAMBLE, is_compaction_text, message_text, system_text,
};

pub(in crate::anthropic) fn subscription_request_prompt(request: &MessagesRequest) -> String {
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

pub(in crate::anthropic) fn request_json_schema(output_config: &Value) -> Option<String> {
    let format = output_config.get("format")?;
    if format.get("type").and_then(Value::as_str) != Some("json_schema") {
        return None;
    }
    serde_json::to_string(format.get("schema")?.as_object()?).ok()
}

pub(in crate::anthropic) fn is_compaction_request(request: &MessagesRequest) -> bool {
    let Some(message) = request.messages.last() else {
        return false;
    };
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = message_text(message.get("content").unwrap_or(&Value::Null));
    is_compaction_text(text.trim_start())
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

pub(in crate::anthropic) fn subscription_request_cwd(request: &MessagesRequest) -> Option<PathBuf> {
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
