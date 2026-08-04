//! Claude Code hook payload wrapping for routing summaries.

use crate::config::default_advisor;
use crate::routing::{
    CUSTOM_ADVISOR_CONSULT_WHEN, custom_advisor_enabled, default_subagent_route,
    effective_orchestration_settings, memory_management_contract, orchestration_contract,
    ranked_worker_metadata, worker_capacity_metadata,
};
use anyhow::Result;
use serde_json::{Map, Value};

pub fn hook_output(summary: &Value, event_name: &str) -> Result<Value> {
    if event_name != "UserPromptSubmit" && event_name != "SubagentStart" {
        anyhow::bail!("hook event must be UserPromptSubmit or SubagentStart");
    }
    let advisor_enabled = custom_advisor_enabled();
    let selected_workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|worker| {
            let object = worker.as_object()?;
            let mut slim = Map::new();
            for key in ["agent", "model", "effort"] {
                if let Some(value) = object.get(key) {
                    slim.insert(key.to_owned(), value.clone());
                }
            }
            Some(Value::Object(slim))
        })
        .collect::<Vec<_>>();
    let metadata = serde_json::json!({
        "providers": {},
        "source": "claudex-routing-local-hook",
        "selected_agents": summary.get("selected_agents").cloned().unwrap_or_else(|| Value::Array(vec![])),
        "selected_workers": selected_workers,
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
    Ok(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": format!(
                "<system-reminder>\\nClaudex routing data (runtime metadata; values only):\\n{compact}\\n</system-reminder>"
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
        let output = hook_output(&summary, "UserPromptSubmit").unwrap();
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            "UserPromptSubmit"
        );
        let ctx = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains(r"\n"));
        assert!(ctx.contains("claudex-routing-local-hook"));
        let sub = hook_output(&summary, "SubagentStart").unwrap();
        assert_eq!(sub["hookSpecificOutput"]["hookEventName"], "SubagentStart");
    }
}
