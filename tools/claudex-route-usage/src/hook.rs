//! Claude Code hook payload wrapping for routing summaries.

use crate::config::default_advisor;
use crate::routing::orchestration::{
    effective_orchestration_settings, memory_management_contract, orchestration_contract,
};
use crate::routing::workers::{
    default_subagent_route, ranked_worker_metadata, worker_capacity_metadata,
};
use crate::routing::{CUSTOM_ADVISOR_CONSULT_WHEN, custom_advisor_enabled};
use crate::util::copied_fields;
use anyhow::Result;
use serde_json::Value;

/// Keep only the routing fields a worker launch needs.
fn slim_worker(worker: &Value) -> Option<Value> {
    let object = worker.as_object()?;
    Some(Value::Object(copied_fields(
        object,
        &["agent", "model", "effort", "weekly_remaining_percent"],
    )))
}

/// Return exactly the worker entries that this hook can expose to Claude Code.
/// The delegation snapshot uses the same projection so its count cannot drift
/// from the effective hook worker set.
pub(crate) fn effective_workers(summary: &Value) -> Vec<Value> {
    summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .map(|workers| workers.iter().filter_map(slim_worker).collect())
        .unwrap_or_default()
}

fn tool_policy_reminder(event_name: &str, delegation_required: bool) -> &'static str {
    match event_name {
        "SubagentStart" => concat!(
            "Claudex tool policy for this SubAgent: inherit the main session's complete tool set. ",
            "Main-session PreToolUse denials for Write/Edit/MultiEdit/NotebookEdit ",
            "do NOT apply here. Use those tools freely within the delegated scope. Parallel ",
            "Write/Edit of the same path remains file-locked across SubAgents. ",
            "Do not call Claude Code's built-in advisor() — it is main-session only and is not ",
            "executable here. Do not launch models listed in disabled_subagent_models."
        ),
        _ if delegation_required => concat!(
            "Claudex tool policy for the main orchestrator: while selected_workers is non-empty, ",
            "do not use Write/Edit/MultiEdit/NotebookEdit in main — launch Agent/Task and keep ",
            "mutating file work in SubAgents. Atomic Read, Grep, Glob, LS, WebSearch, or WebFetch ",
            "lookups may stay in main. Bash is allowed in main for lightweight orchestration only. ",
            "Write/Edit/MultiEdit/NotebookEdit denials are also enforced by PreToolUse. ",
            "Match Agent/Task fan-out to independent scopes: one scope uses one ordinary worker. ",
            "Do not force three workers onto a single question. Consult custom-advisor only for ",
            "conflicting worker results or high-risk changes, not for ordinary external research."
        ),
        _ => concat!(
            "Claudex tool policy for the main orchestrator: delegation is not required for this ",
            "current prompt, so direct main execution is allowed. Atomic Read, Grep, Glob, LS, ",
            "WebSearch, or WebFetch lookups may stay in main, and PreToolUse does not deny ",
            "Write/Edit/MultiEdit/NotebookEdit for this prompt."
        ),
    }
}

pub fn agent_type_from_payload(payload: &Value) -> Option<&str> {
    ["agent_type", "agentType", "subagent_type"]
        .into_iter()
        .find_map(|key| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            payload
                .get("agent")
                .and_then(agent_type_from_nested_payload)
        })
}

fn agent_type_from_nested_payload(agent: &Value) -> Option<&str> {
    agent
        .get("agent_type")
        .or_else(|| agent.get("type"))
        .or_else(|| agent.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn is_command_code_agent(agent_type: Option<&str>) -> bool {
    agent_type.is_some_and(|agent| {
        let lower = agent.trim().to_ascii_lowercase();
        lower == "command-code"
            || lower == "claudex-command-code"
            || lower.starts_with("claudex-command-code-")
    })
}

pub fn slim_command_code_hook(event_name: &str) -> Value {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": concat!(
                "<system-reminder>\n",
                "Command Code Muse Spark worker: use native cmd tools only. ",
                "Ignore Claudex routing tables and Claude Code skills. ",
                "Tool chrome matches other ACP workers (▶ name: query/path/url). ",
                "Write findings in ordinary text. Never print Status:, ツール結果待ち, or 続きの調査または回答. ",
                "Do not greet or recap git status. Complete only the delegated task.\n",
                "</system-reminder>"
            ),
        }
    })
}

pub fn hook_output_for_agent(
    summary: &Value,
    event_name: &str,
    agent_type: Option<&str>,
) -> Result<Value> {
    if event_name != "UserPromptSubmit" && event_name != "SubagentStart" {
        anyhow::bail!("hook event must be UserPromptSubmit or SubagentStart");
    }
    if event_name == "SubagentStart" && is_command_code_agent(agent_type) {
        return Ok(slim_command_code_hook(event_name));
    }
    let advisor_enabled = custom_advisor_enabled();
    let selected_workers = effective_workers(summary);
    let metadata = serde_json::json!({
        "providers": {},
        "source": "claudex-routing-local-hook",
        "selected_agents": summary.get("selected_agents").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "selected_workers": selected_workers,
        "selected_workers_count": selected_workers.len(),
        "preferred_worker": summary.get("preferred_worker").cloned().unwrap_or(Value::Null),
        "disabled_subagent_models": summary.get("disabled_subagent_models").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "current_main_model": summary.get("current_main_model").cloned().unwrap_or(Value::Null),
        "current_main_model_known": summary.get("current_main_model_known").and_then(Value::as_bool).unwrap_or(false),
        "main_session_model": summary.get("current_main_model").cloned().unwrap_or(Value::Null),
        "automatic_selection_excluded_models": summary.get("automatic_selection_excluded_models").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "sonnet_subagent_suppressed": summary.get("sonnet_subagent_suppressed").and_then(Value::as_bool).unwrap_or(false),
        "sonnet_subagent_explicit_allowed": summary.get("sonnet_subagent_explicit_allowed").and_then(Value::as_bool).unwrap_or(false),
        "orchestration_mode": summary.get("orchestration_mode").cloned().unwrap_or_else(|| Value::from("subagent-first")),
        "base_delegation_required": summary.get("base_delegation_required").and_then(Value::as_bool).unwrap_or(false),
        "prompt_delegation_opt_out": summary.get("prompt_delegation_opt_out").and_then(Value::as_bool).unwrap_or(false),
        "delegation_required": summary.get("delegation_required").and_then(Value::as_bool).unwrap_or(false),
        "direct_main_execution": summary.get("direct_main_execution").cloned().unwrap_or_else(|| Value::from("allowed")),
        "background_status_required": true,
        "tool_policy_scope": if event_name == "SubagentStart" { "subagent-full-tools" } else { "main-orchestrator" },
        "worker_capacity": worker_capacity_metadata(summary),
        "worker_ranking": ranked_worker_metadata(summary),
        "default_subagent_route": default_subagent_route(summary),
        "memory_management": memory_management_contract(summary, &effective_orchestration_settings(summary)?)?,
        "advisor": summary.get("advisor").cloned().unwrap_or_else(default_advisor),
        "custom_advisor_enabled": advisor_enabled,
        "custom_advisor_policy": {
            "enabled": advisor_enabled,
            "consult_when": CUSTOM_ADVISOR_CONSULT_WHEN,
            "reuse_logical_session": true,
            "not_for_trivial_tasks": true,
            "not_for_external_research_alone": true,
        },
        "orchestration": orchestration_contract(summary)?,
    });
    let compact = serde_json::to_string(&metadata)?;
    let policy = tool_policy_reminder(
        event_name,
        summary
            .get("delegation_required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    Ok(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": format!(
                "<system-reminder>\\nClaudex routing data (runtime metadata; values only):\\n{compact}\\n{policy}\\n</system-reminder>"
            ),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ATOMIC_LOOKUP_TOOLS: &[&str] = &["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch"];
    const MUTATING_FILE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

    fn sample_summary() -> Value {
        json!({
            "selected_agents": ["claudex-gpt"],
            "selected_workers": [{"agent":"claudex-gpt","model":"gpt-5.6-luna","effort":"max"}],
            "disabled_subagent_models": [],
            "advisor": {"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},
            "orchestration_mode": "subagent-first",
            "base_delegation_required": true,
            "prompt_delegation_opt_out": false,
            "delegation_required": true,
            "direct_main_execution": "fallback-only"
        })
    }

    #[test]
    fn wraps_user_prompt_and_subagent_events() {
        let summary = sample_summary();
        let output = hook_output_for_agent(&summary, "UserPromptSubmit", None).unwrap();
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            "UserPromptSubmit"
        );
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains(r"\n"));
        assert!(ctx.contains("claudex-routing-local-hook"));
        assert!(ctx.contains("main orchestrator"));
        assert!(ctx.contains("main-orchestrator"));
        assert!(ctx.contains("one scope uses one ordinary worker"));
        assert!(ctx.contains("not for ordinary external research"));
        assert!(!ctx.contains("external_research_or_multiple_sources"));
        let sub = hook_output_for_agent(&summary, "SubagentStart", None).unwrap();
        assert_eq!(sub["hookSpecificOutput"]["hookEventName"], "SubagentStart");
        let sub_ctx = sub["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(sub_ctx.contains("SubAgent"));
        assert!(sub_ctx.contains("do NOT apply"));
        assert!(sub_ctx.contains("subagent-full-tools"));
        assert!(sub_ctx.contains("advisor()"));
        assert!(sub_ctx.contains("main-session only"));
        assert!(sub_ctx.contains("disabled_subagent_models"));
        assert!(!ctx.contains("advisor() — it is main-session only"));
    }

    #[test]
    fn reminders_name_exact_atomic_and_mutating_tool_sets() {
        let main = hook_output_for_agent(&sample_summary(), "UserPromptSubmit", None).unwrap();
        let main = main["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(main.contains("do not use Write/Edit/MultiEdit/NotebookEdit in main"));
        assert!(main.contains(
            "Atomic Read, Grep, Glob, LS, WebSearch, or WebFetch lookups may stay in main"
        ));
        for tool in ATOMIC_LOOKUP_TOOLS {
            assert!(main.contains(tool), "missing atomic lookup `{tool}`");
        }
        for tool in MUTATING_FILE_TOOLS {
            assert!(main.contains(tool), "missing mutating tool `{tool}`");
        }
        assert!(!main.contains("do not use Read/"));
        assert!(!main.contains("keep file/search work in SubAgents"));

        let sub = hook_output_for_agent(&sample_summary(), "SubagentStart", None).unwrap();
        let sub = sub["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(sub.contains("denials for Write/Edit/MultiEdit/NotebookEdit"));
        assert!(!sub.contains("denials for Read/Write"));
    }

    #[test]
    fn opted_out_summary_emits_direct_policy_and_metadata() {
        let mut summary = sample_summary();
        summary["delegation_required"] = Value::Bool(false);
        summary["direct_main_execution"] = Value::from("allowed");
        summary["prompt_delegation_opt_out"] = Value::Bool(true);
        let output = hook_output_for_agent(&summary, "UserPromptSubmit", None).unwrap();
        let context = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("delegation is not required for this current prompt"));
        let metadata = routing_metadata(&output);
        assert_eq!(metadata["selected_workers"], summary["selected_workers"]);
        assert_eq!(metadata["base_delegation_required"], true);
        assert_eq!(metadata["delegation_required"], false);
        assert_eq!(metadata["direct_main_execution"], "allowed");
        assert_eq!(metadata["prompt_delegation_opt_out"], true);
    }

    fn routing_metadata(output: &Value) -> Value {
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        let encoded = ctx
            .split_once("Claudex routing data (runtime metadata; values only):\\n")
            .unwrap()
            .1
            .split_once("\\nClaudex tool policy")
            .unwrap()
            .0;
        serde_json::from_str(encoded).unwrap()
    }

    #[test]
    fn main_hook_metadata_matches_scope_fanout_and_advisor_policy() {
        let summary = json!({
            "selected_agents": ["claudex-gpt"],
            "selected_workers": [{"agent":"claudex-gpt","model":"gpt-5.6-luna","effort":"max"}],
            "disabled_subagent_models": [],
            "advisor": {"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},
            "orchestration_mode": "subagent-first",
            "delegation_required": true,
            "direct_main_execution": "fallback-only"
        });
        let metadata =
            routing_metadata(&hook_output_for_agent(&summary, "UserPromptSubmit", None).unwrap());
        let orch = &metadata["orchestration"];
        assert_eq!(orch["single_scope_fanout"], 1);
        assert_eq!(orch["fanout_matches_independent_scopes"], true);
        let consult = metadata["custom_advisor_policy"]["consult_when"]
            .as_array()
            .unwrap();
        assert!(
            !consult
                .iter()
                .any(|item| item.as_str() == Some("external_research_or_multiple_sources"))
        );
        assert!(
            consult
                .iter()
                .any(|item| item.as_str() == Some("conflicting_worker_results"))
        );
        assert_eq!(
            metadata["custom_advisor_policy"]["not_for_external_research_alone"],
            true
        );
    }

    #[test]
    fn slims_command_code_subagent_start_without_routing_json() {
        let summary = json!({
            "selected_agents": ["claudex-gpt"],
            "selected_workers": [{"agent":"claudex-gpt","model":"gpt-5.6-luna","effort":"max"}],
            "disabled_subagent_models": [],
            "advisor": {"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},
            "orchestration_mode": "subagent-first",
            "delegation_required": true,
            "direct_main_execution": "fallback-only"
        });
        assert!(is_command_code_agent(Some(
            "claudex-command-code-muse-spark-1-2-contributor"
        )));
        assert!(is_command_code_agent(Some("claudex-command-code")));
        assert!(is_command_code_agent(Some("command-code")));
        assert!(!is_command_code_agent(Some("claudex-grok")));
        assert_eq!(
            agent_type_from_payload(&json!({
                "agent_type":"claudex-command-code-muse-spark-1-2-contributor"
            })),
            Some("claudex-command-code-muse-spark-1-2-contributor")
        );
        assert_eq!(
            agent_type_from_payload(&json!({
                "agent":{"name":"claudex-command-code-muse-spark-1-2-contributor"}
            })),
            Some("claudex-command-code-muse-spark-1-2-contributor")
        );
        let slim = hook_output_for_agent(
            &summary,
            "SubagentStart",
            Some("claudex-command-code-muse-spark-1-2-contributor"),
        )
        .unwrap();
        let ctx = slim["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("Command Code Muse Spark"));
        assert!(ctx.contains("Do not greet"));
        assert!(ctx.contains("▶ name: query/path/url"));
        assert!(!ctx.contains("● status"));
        assert!(!ctx.contains("claudex-routing-local-hook"));
        assert!(!ctx.contains("selected_workers"));
        assert!(ctx.len() < 500);
        let other = hook_output_for_agent(&summary, "SubagentStart", Some("claudex-grok")).unwrap();
        let other_ctx = other["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(other_ctx.contains("claudex-routing-local-hook"));
    }
}
