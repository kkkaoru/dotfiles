use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[cfg(test)]
thread_local! {
    static SETTINGS_FILE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

mod command;
mod effort;
pub(super) mod failure;
mod lifecycle;
mod options;
mod retry;
mod run;
pub(in crate::anthropic) use command::{OutputMode, subscription_command};
#[cfg(test)]
pub(super) use effort::request_effort;
pub(super) use effort::valid_effort;

#[cfg(test)]
pub(super) use super::subscription_request::cwd_from_system;
#[cfg(test)]
pub(super) use super::subscription_request::requested_tools;
pub(super) use super::subscription_request::requested_tools_for_request;
use super::{
    Bridge, MessagesRequest, Segment, Usage, WebEvidenceSummary,
    content::{anthropic_response, estimated_tokens, token_count},
    subscription_request::{
        is_compaction_request, request_json_schema, subscription_request_cwd,
        subscription_request_prompt,
    },
    subscription_stream::{
        ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
        subscription_streaming_response,
    },
};
pub(super) use failure::{
    subscription_failure, subscription_result_text, validate_subscription_result_for_model,
};
#[cfg(test)]
pub(super) use failure::{subscription_result, validate_subscription_result};
#[cfg(test)]
pub(in crate::anthropic) use lifecycle::terminate_subscription;
pub(in crate::anthropic) use lifecycle::terminate_subscription_process_group;
pub(super) use options::{
    DEFAULT_STDERR_DRAIN_GRACE, DEFAULT_TERMINATION_TIMEOUT, SubscriptionOptions,
    SubscriptionToolContext,
};
pub(super) use retry::with_transient_retries;
#[cfg(test)]
pub(super) use retry::{should_retry_subscription, transient_retry_delay};
pub(super) use run::{
    acquire_subscription_slot, run_subscription_model, spawn_subscription, take_subscription_stdin,
    write_subscription_prompt,
};

#[path = "subscription_limits.rs"]
mod limits;
#[allow(unused_imports)]
pub(in crate::anthropic) use limits::subscription_limits_from;
pub(crate) use limits::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES};
pub(in crate::anthropic) use limits::{SubscriptionLimits, subscription_limits};
#[allow(unused_imports)] // settings_tests via super::
pub(in crate::anthropic) use limits::{positive_u64, positive_usize};

impl Bridge {
    pub(super) fn claude_settings(&self) -> ClaudeSettings {
        self.settings_path
            .as_deref()
            .and_then(settings_at)
            .unwrap_or_default()
    }

    pub(super) fn claude_setting(&self, key: &str) -> Option<String> {
        self.claude_settings().get(key)
    }

    pub(super) fn claude_effort(&self) -> Option<String> {
        self.claude_setting("effortLevel")
            .filter(|effort| valid_effort(effort))
    }
}

#[derive(Default)]
pub(super) struct ClaudeSettings {
    values: Map<String, Value>,
}

impl ClaudeSettings {
    pub(super) fn get(&self, key: &str) -> Option<String> {
        self.values
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }
}

#[path = "subscription_bridge.rs"]
mod bridge;

pub(super) fn claude_settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude/settings.json"))
}

#[cfg(test)]
pub(super) fn setting_at(path: &Path, key: &str) -> Option<String> {
    settings_at(path)?.get(key)
}

/// Read and parse Claude settings once when several values are needed for one
/// turn.  Settings remain live: each caller still reads the latest file.
pub(super) fn settings_at(path: &Path) -> Option<ClaudeSettings> {
    #[cfg(test)]
    SETTINGS_FILE_READS.with(|count| count.set(count.get() + 1));
    let settings = std::fs::read(path).ok()?;
    serde_json::from_slice::<Map<String, Value>>(&settings)
        .ok()
        .map(|values| ClaudeSettings { values })
}

/// Run a test operation while counting synchronous settings-file reads on this
/// thread. Thread-local state keeps the count deterministic under parallel tests.
#[cfg(test)]
pub(super) fn count_settings_file_reads<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    SETTINGS_FILE_READS.with(|count| {
        let prior = count.replace(0);
        let result = operation();
        let reads = count.replace(prior);
        (result, reads)
    })
}

#[cfg(test)]
pub(super) fn subscription_prompt(tool: &str, arguments: &Value, transcript: &[Value]) -> String {
    if tool == "advisor" {
        return format!(
            "Act as a rigorous advisor. Review the complete conversation below and return concise, actionable guidance to the primary coding agent. Do not use tools.\n\n{}",
            serde_json::to_string(transcript).unwrap_or_default()
        );
    }
    format!(
        "Work as an independent Claude collaborator. Complete the delegated task using the supplied conversation context. Do not use tools.\n\nTask:\n{}\n\nConversation:\n{}",
        arguments
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("Review the conversation and suggest the next step."),
        serde_json::to_string(transcript).unwrap_or_default()
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "subscription_settings_tests.rs"]
mod settings_tests;
