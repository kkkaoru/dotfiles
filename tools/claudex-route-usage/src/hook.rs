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

fn tool_policy_reminder(event_name: &str) -> &'static str {
    match event_name {
        "SubagentStart" => concat!(
            "Claudex tool policy for this SubAgent: inherit the main session's complete tool set. ",
            "Main-session PreToolUse denials for Read/Write/Edit/Grep/Glob/LS/WebSearch/WebFetch ",
            "do NOT apply here. Use those tools freely within the delegated scope. Parallel ",
            "Write/Edit of the same path remains file-locked across SubAgents. ",
            "Do not call Claude Code's built-in advisor() — it is main-session only and is not ",
            "executable here. Do not launch models listed in disabled_subagent_models."
        ),
        _ => concat!(
            "Claudex tool policy for the main orchestrator: while selected_workers is non-empty, ",
            "do not use Read/Write/Edit/MultiEdit/NotebookEdit/Grep/Glob/LS/WebSearch/WebFetch ",
            "in main — launch Agent/Task and keep file/search work in SubAgents. Bash is allowed ",
            "in main for lightweight orchestration only. This is also enforced by PreToolUse."
        ),
    }
}

pub fn agent_type_from_payload(payload: &Value) -> Option<&str> {
    for key in ["agent_type", "agentType", "subagent_type"] {
        if let Some(value) = payload.get(key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    payload.get("agent").and_then(|agent| {
        agent
            .get("agent_type")
            .or_else(|| agent.get("type"))
            .or_else(|| agent.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
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
    let selected_workers: Vec<Value> = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .map(|workers| workers.iter().filter_map(slim_worker).collect())
        .unwrap_or_default();
    let metadata = serde_json::json!({
        "providers": {},
        "source": "claudex-routing-local-hook",
        "selected_agents": summary.get("selected_agents").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "selected_workers": selected_workers,
        "preferred_worker": summary.get("preferred_worker").cloned().unwrap_or(Value::Null),
        "disabled_subagent_models": summary.get("disabled_subagent_models").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "current_main_model": summary.get("current_main_model").cloned().unwrap_or(Value::Null),
        "current_main_model_known": summary.get("current_main_model_known").and_then(Value::as_bool).unwrap_or(false),
        "main_session_model": summary.get("current_main_model").cloned().unwrap_or(Value::Null),
        "automatic_selection_excluded_models": summary.get("automatic_selection_excluded_models").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "sonnet_subagent_suppressed": summary.get("sonnet_subagent_suppressed").and_then(Value::as_bool).unwrap_or(false),
        "sonnet_subagent_explicit_allowed": summary.get("sonnet_subagent_explicit_allowed").and_then(Value::as_bool).unwrap_or(false),
        "orchestration_mode": summary.get("orchestration_mode").cloned().unwrap_or_else(|| Value::from("subagent-first")),
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
        },
        "orchestration": orchestration_contract(summary)?,
    });
    let compact = serde_json::to_string(&metadata)?;
    let policy = tool_policy_reminder(event_name);
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

    #[test]
    fn wraps_user_prompt_and_subagent_events() {
        let summary = json!({
            "selected_agents": ["claudex-gpt"],
            "selected_workers": [{"agent":"claudex-gpt","model":"gpt-5.6-luna","effort":"max"}],
            "disabled_subagent_models": [],
            "advisor": {"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},
            "orchestration_mode": "subagent-first",
            "delegation_required": true,
            "direct_main_execution": "fallback-only"
        });
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
