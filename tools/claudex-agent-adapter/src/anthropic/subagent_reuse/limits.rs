use serde_json::Value;

use super::super::MessagesRequest;
use super::store::METADATA_LIMIT_REACHED;

pub(in crate::anthropic) const DEFAULT_MAX_SUBAGENTS: usize =
    crate::parallel_scheduler::DEFAULT_MAX_PARALLEL_WORKERS;

pub(in crate::anthropic) const NESTED_SUBAGENT_LAUNCH_NOTICE: &str = "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker.";

pub(in crate::anthropic) fn max_subagents_per_session() -> usize {
    std::env::var(super::MAX_SUBAGENTS_PER_SESSION_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_SUBAGENTS)
}

pub(in crate::anthropic) fn should_expose_launch_tools(request: &MessagesRequest) -> bool {
    if crate::anthropic::agent_effort::is_subagent_request(request) {
        return false;
    }
    request
        .metadata
        .get(METADATA_LIMIT_REACHED)
        .and_then(Value::as_bool)
        .is_none_or(|reached| !reached)
}

pub(in crate::anthropic) fn is_launch_tool(name: &str) -> bool {
    if matches!(name, "Agent" | "Task") || name.to_ascii_lowercase().contains("spawn_subagent") {
        return true;
    }
    let Some(rest) = name.strip_prefix("cc_") else {
        return false;
    };
    matches!(rest.split('_').next().unwrap_or(rest), "Agent" | "Task")
}

pub(in crate::anthropic) fn nested_subagent_launch_notice(
    tool_name: &str,
    is_subagent: bool,
) -> Option<&'static str> {
    (is_subagent && is_launch_tool(tool_name)).then_some(NESTED_SUBAGENT_LAUNCH_NOTICE)
}

pub(in crate::anthropic) fn reuse_enabled() -> bool {
    match std::env::var(crate::parallel_scheduler::SUBAGENT_REUSE_ENV) {
        Ok(value) => matches!(
            value.as_str(),
            "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => true,
    }
}

pub(in crate::anthropic) fn session_id(request: &MessagesRequest) -> Option<String> {
    super::super::request_identity::claude_session_id(request)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::{Value, json};

    use super::super::super::MessagesRequest;
    use super::{
        DEFAULT_MAX_SUBAGENTS, is_launch_tool, nested_subagent_launch_notice,
        should_expose_launch_tools,
    };

    #[test]
    fn default_live_agent_cap_matches_scheduler_max_parallel() {
        assert_eq!(DEFAULT_MAX_SUBAGENTS, 40);
    }

    #[test]
    fn nested_subagent_agent_and_task_return_the_english_notice() {
        assert_eq!(
            nested_subagent_launch_notice("Agent", true),
            Some(
                "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
            )
        );
        assert_eq!(
            nested_subagent_launch_notice("Task", true),
            Some(
                "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
            )
        );
        assert_eq!(
            nested_subagent_launch_notice("spawn_subagent", true),
            Some(
                "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
            )
        );
        assert_eq!(
            nested_subagent_launch_notice("cc_Agent_0", true),
            Some(
                "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
            )
        );
        assert_eq!(
            nested_subagent_launch_notice("cc_Task_1", true),
            Some(
                "Nested SubAgent sessions must not launch Agent/Task. The parent session owns fan-out. Continue the delegated task with the tools you have. SendMessage({to}) is still allowed to continue an existing worker."
            )
        );
    }

    #[test]
    fn send_message_and_task_lifecycle_are_not_nested_launch_rejections() {
        assert_eq!(nested_subagent_launch_notice("SendMessage", true), None);
        assert_eq!(
            nested_subagent_launch_notice("cc_SendMessage_3", true),
            None
        );
        assert_eq!(nested_subagent_launch_notice("TaskOutput", true), None);
        assert_eq!(nested_subagent_launch_notice("cc_TaskOutput_0", true), None);
        assert_eq!(nested_subagent_launch_notice("TaskStop", true), None);
        assert_eq!(nested_subagent_launch_notice("Read", true), None);
        assert_eq!(nested_subagent_launch_notice("Agent", false), None);
        assert_eq!(nested_subagent_launch_notice("Task", false), None);
    }

    #[test]
    fn launch_tools_include_spawn_subagent_and_cc_agent_names() {
        assert!(is_launch_tool("Agent"));
        assert!(is_launch_tool("Task"));
        assert!(is_launch_tool("spawn_subagent"));
        assert!(is_launch_tool("Spawn_Subagent"));
        assert!(is_launch_tool("MCP__spawn_subagent"));
        assert!(is_launch_tool("cc_Agent_0"));
        assert!(is_launch_tool("cc_Task_1"));
        assert!(!is_launch_tool("SendMessage"));
        assert!(!is_launch_tool("cc_SendMessage_3"));
        assert!(!is_launch_tool("TaskOutput"));
        assert!(!is_launch_tool("Read"));
    }

    #[test]
    fn nested_subagent_sessions_hide_agent_and_task_launch_tools() {
        let nested = MessagesRequest {
            model: "worker".to_owned(),
            system: json!("cc_is_subagent=true"),
            messages: vec![json!({"role":"user","content":"do the work"})],
            tools: vec![
                json!({"name":"Agent"}),
                json!({"name":"spawn_subagent"}),
                json!({"name":"SendMessage"}),
            ],
            stream: false,
            output_config: Value::Null,
            metadata: json!({}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };
        assert!(!should_expose_launch_tools(&nested));
        assert!(super::is_launch_tool("Agent"));
        assert!(super::is_launch_tool("Task"));
        assert!(super::is_launch_tool("spawn_subagent"));
        assert!(super::is_launch_tool("mcp__spawn_subagent"));
        assert!(!super::is_launch_tool("SendMessage"));
        assert!(!super::is_launch_tool("TaskOutput"));
        assert!(!super::is_launch_tool("Bash"));

        let main = MessagesRequest {
            model: "main".to_owned(),
            system: json!("main session"),
            messages: vec![json!({"role":"user","content":"delegate work"})],
            tools: vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})],
            stream: false,
            output_config: Value::Null,
            metadata: json!({}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };
        assert!(should_expose_launch_tools(&main));
    }
}
