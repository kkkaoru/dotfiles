//! Nested SubAgent sessions must not launch more Agent/Task workers.

use serde_json::Value;

use super::agent_route_validation::BlockedSubagentError;
use super::subscription::SubscriptionToolContext;

pub(in crate::anthropic) fn is_forbidden_nested_launch_name(name: &str) -> bool {
    super::subagent_reuse::is_launch_tool(name)
}

pub(in crate::anthropic) fn event_is_forbidden_nested_launch(event: &Value) -> bool {
    let Some(params) = event.get("params") else {
        return false;
    };
    if super::subagent_reuse::is_send_message_follow_up(
        params.get("arguments").unwrap_or(&Value::Null),
    ) {
        return false;
    }
    name_is_forbidden(params.get("tool").and_then(Value::as_str))
        || name_is_forbidden(params.get("title").and_then(Value::as_str))
        || name_is_forbidden(
            params
                .pointer("/arguments/_toolName")
                .and_then(Value::as_str),
        )
}

pub(in crate::anthropic) fn reject_if_nested_agent_launch(
    context: &SubscriptionToolContext,
    arguments: &Value,
) -> Result<(), BlockedSubagentError> {
    if super::subagent_reuse::should_reject_nested_launch(context.is_subagent, arguments) {
        return Err(BlockedSubagentError::nested_launch());
    }
    Ok(())
}

fn name_is_forbidden(name: Option<&str>) -> bool {
    name.is_some_and(is_forbidden_nested_launch_name)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn forbids_agent_task_and_spawn_subagent_names_only() {
        assert!(is_forbidden_nested_launch_name("Agent"));
        assert!(is_forbidden_nested_launch_name("Task"));
        assert!(is_forbidden_nested_launch_name("spawn_subagent"));
        assert!(is_forbidden_nested_launch_name("Spawn_Subagent"));
        assert!(is_forbidden_nested_launch_name("cc_Agent_0"));
        assert!(is_forbidden_nested_launch_name("cc_Task_1"));
        assert!(!is_forbidden_nested_launch_name("SendMessage"));
        assert!(!is_forbidden_nested_launch_name("cc_SendMessage_3"));
        assert!(!is_forbidden_nested_launch_name("TaskOutput"));
        assert!(!is_forbidden_nested_launch_name("TaskStop"));
        assert!(!is_forbidden_nested_launch_name("TaskGet"));
        assert!(!is_forbidden_nested_launch_name("Read"));
    }

    #[test]
    fn detects_forbidden_nested_launch_events_and_allows_send_message() {
        assert!(event_is_forbidden_nested_launch(&json!({
            "params":{"tool":"Agent","arguments":{"prompt":"work"}}
        })));
        assert!(event_is_forbidden_nested_launch(&json!({
            "params":{"title":"Task","arguments":{"prompt":"work"}}
        })));
        assert!(event_is_forbidden_nested_launch(&json!({
            "params":{"tool":"mcp","arguments":{"_toolName":"spawn_subagent","prompt":"work"}}
        })));
        assert!(event_is_forbidden_nested_launch(&json!({
            "params":{"tool":"cc_Agent_0","arguments":{"prompt":"work"}}
        })));
        assert!(!event_is_forbidden_nested_launch(&json!({
            "params":{"tool":"SendMessage","arguments":{"to":"a0123456789abcdef","message":"go"}}
        })));
        assert!(!event_is_forbidden_nested_launch(&json!({
            "params":{
                "tool":"Agent",
                "arguments":{"to":"a0123456789abcdef","message":"go"}
            }
        })));
        assert!(!event_is_forbidden_nested_launch(&json!({
            "params":{"tool":"Read","arguments":{"path":"src/lib.rs"}}
        })));
        assert!(!event_is_forbidden_nested_launch(&json!({})));
    }

    #[test]
    fn nested_subscription_context_is_rejected_with_the_english_notice() {
        let mut context = SubscriptionToolContext::for_tests(
            std::sync::Arc::new(crate::anthropic::agent_effort::AgentEffortIntents::default()),
            crate::provider_config::ModelCatalog::default(),
            None,
            "parent-model",
            Vec::new(),
            Value::Null,
        );
        reject_if_nested_agent_launch(&context, &json!({"prompt":"work"}))
            .expect("main session may launch");
        context.is_subagent = true;
        reject_if_nested_agent_launch(
            &context,
            &json!({"to":"a0123456789abcdef","message":"continue"}),
        )
        .expect("nested SendMessage({to}) remains allowed");
        let error = reject_if_nested_agent_launch(&context, &json!({"prompt":"work"}))
            .expect_err("nested must reject");
        assert_eq!(
            error.notice(),
            "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
        );
    }
}
