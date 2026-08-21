use serde_json::Value;

use crate::anthropic::MessagesRequest;

#[cfg(test)]
pub(in crate::anthropic) fn requested_tools(
    tools: &[Value],
    omit_task_bookkeeping: bool,
) -> Vec<String> {
    requested_tools_from_request(tools, omit_task_bookkeeping, true)
}

pub(in crate::anthropic) fn requested_tools_for_request(
    request: &MessagesRequest,
    omit_task_bookkeeping: bool,
) -> Vec<String> {
    let hide_main_only_tools = crate::anthropic::agent_effort::is_subagent_request(request);
    let mut provider_tools = request.tools.clone();
    if hide_main_only_tools {
        provider_tools.retain(|tool| {
            !tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(crate::anthropic::session::is_main_session_only_tool)
        });
    }
    requested_tools_from_request(
        &provider_tools,
        omit_task_bookkeeping,
        crate::anthropic::subagent_reuse::should_expose_launch_tools(request),
    )
}

pub(super) fn requested_tools_from_request(
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
