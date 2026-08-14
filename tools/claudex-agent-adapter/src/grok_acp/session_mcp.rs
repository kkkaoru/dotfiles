use std::{env, path::PathBuf};

use agent_client_protocol as acp;
use serde_json::Value;

use super::{LAUNCH_MCP_COMMAND, LAUNCH_MCP_NAME};

pub(super) fn launch_mcp_servers(params: &Value) -> Vec<acp::McpServer> {
    launch_mcp_servers_from(params, env::current_exe())
}

pub(super) fn launch_mcp_servers_from(
    params: &Value,
    exe: std::io::Result<PathBuf>,
) -> Vec<acp::McpServer> {
    let dynamic_tools_metrics = dynamic_tools_metrics(params);
    let dynamic_tools = dynamic_tools_metrics.state;
    tracing::info!(
        dynamic_tools = dynamic_tools.as_str(),
        dynamic_tools_shape = dynamic_tools_metrics.shape.as_str(),
        dynamic_tools_is_array = dynamic_tools_metrics.is_array,
        dynamic_tools_count = dynamic_tools_metrics.tool_count,
        dynamic_tools_has_exact_agent = dynamic_tools_metrics.has_exact_agent,
        dynamic_tools_has_exact_task = dynamic_tools_metrics.has_exact_task,
        dynamic_tools_has_case_insensitive_agent = dynamic_tools_metrics.has_case_insensitive_agent,
        dynamic_tools_has_case_insensitive_task = dynamic_tools_metrics.has_case_insensitive_task,
        dynamic_tools_launch_like_count = dynamic_tools_metrics.launch_like_count,
        "ACP launch MCP eligibility evaluated"
    );
    if dynamic_tools != DynamicToolsState::Matching {
        return Vec::new();
    }
    let Ok(exe) = exe else {
        tracing::warn!(
            dynamic_tools = dynamic_tools.as_str(),
            "adapter executable unavailable; ACP Agent/Task tools not injected"
        );
        return Vec::new();
    };
    let cache = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache/claudex");
    let log_path = cache.join("claudex-launch-mcp.log");
    let owner = crate::launch_mcp::launch_owner_from_params(params);
    let queue_path = crate::launch_mcp::launch_queue_path(&cache, owner.as_deref());
    let mut env = vec![
        acp::EnvVariable::new(
            "CLAUDEX_LAUNCH_MCP_LOG",
            log_path.to_string_lossy().into_owned(),
        ),
        acp::EnvVariable::new(
            "CLAUDEX_LAUNCH_QUEUE",
            queue_path.to_string_lossy().into_owned(),
        ),
    ];
    if let Some(owner) = owner {
        env.push(acp::EnvVariable::new("CLAUDEX_LAUNCH_OWNER", owner));
    }
    vec![acp::McpServer::Stdio(
        acp::McpServerStdio::new(LAUNCH_MCP_NAME, exe)
            .args(vec![LAUNCH_MCP_COMMAND.to_owned()])
            .env(env),
    )]
}

#[cfg(test)]
pub(super) fn params_offer_launch_tools(params: &Value) -> bool {
    dynamic_tools_state(params) == DynamicToolsState::Matching
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DynamicToolsState {
    Absent,
    Nonmatching,
    Matching,
}

impl DynamicToolsState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Nonmatching => "nonmatching",
            Self::Matching => "matching",
        }
    }
}

#[cfg(test)]
fn dynamic_tools_state(params: &Value) -> DynamicToolsState {
    dynamic_tools_metrics(params).state
}

/// Aggregates only fixed labels, booleans, and counts for eligibility tracing.
/// `launch_like_count` deliberately counts name-based, case-sensitive `Agent`/
/// `Task` substring matches only; description-only and case-insensitive signals
/// remain separate so this metric cannot broaden the launch gate.
#[derive(Clone, Copy)]
struct DynamicToolsMetrics {
    state: DynamicToolsState,
    shape: DynamicToolsShape,
    is_array: bool,
    tool_count: usize,
    has_exact_agent: bool,
    has_exact_task: bool,
    has_case_insensitive_agent: bool,
    has_case_insensitive_task: bool,
    launch_like_count: usize,
}

#[derive(Clone, Copy)]
enum DynamicToolsShape {
    Absent,
    Malformed,
    Array,
}

impl DynamicToolsShape {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Malformed => "malformed",
            Self::Array => "array",
        }
    }
}

fn dynamic_tools_metrics(params: &Value) -> DynamicToolsMetrics {
    let Some(tools) = params.get("dynamicTools") else {
        return DynamicToolsMetrics {
            state: DynamicToolsState::Absent,
            shape: DynamicToolsShape::Absent,
            is_array: false,
            tool_count: 0,
            has_exact_agent: false,
            has_exact_task: false,
            has_case_insensitive_agent: false,
            has_case_insensitive_task: false,
            launch_like_count: 0,
        };
    };
    let Some(tools) = tools.as_array() else {
        return DynamicToolsMetrics {
            state: DynamicToolsState::Nonmatching,
            shape: DynamicToolsShape::Malformed,
            is_array: false,
            tool_count: 0,
            has_exact_agent: false,
            has_exact_task: false,
            has_case_insensitive_agent: false,
            has_case_insensitive_task: false,
            launch_like_count: 0,
        };
    };
    let mut metrics = DynamicToolsMetrics {
        state: DynamicToolsState::Nonmatching,
        shape: DynamicToolsShape::Array,
        is_array: true,
        tool_count: tools.len(),
        has_exact_agent: false,
        has_exact_task: false,
        has_case_insensitive_agent: false,
        has_case_insensitive_task: false,
        launch_like_count: 0,
    };
    for tool in tools {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
        metrics.has_exact_agent |= name == "Agent";
        metrics.has_exact_task |= name == "Task";
        metrics.has_case_insensitive_agent |= name.eq_ignore_ascii_case("Agent");
        metrics.has_case_insensitive_task |= name.eq_ignore_ascii_case("Task");
        if name.contains("Agent") || name.contains("Task") {
            metrics.launch_like_count += 1;
        }
        if tool_offers_launch(tool) {
            metrics.state = DynamicToolsState::Matching;
        }
    }
    metrics
}

fn tool_offers_launch(tool: &Value) -> bool {
    let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    name == "Agent"
        || name == "Task"
        || name.contains("Agent")
        || name.contains("Task")
        || description.contains("`Agent`")
        || description.contains("`Task`")
}
