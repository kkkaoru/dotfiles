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
    let dynamic_tools = dynamic_tools_state(params);
    tracing::info!(
        dynamic_tools = dynamic_tools.as_str(),
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

fn dynamic_tools_state(params: &Value) -> DynamicToolsState {
    let Some(tools) = params.get("dynamicTools") else {
        return DynamicToolsState::Absent;
    };
    let Some(tools) = tools.as_array() else {
        return DynamicToolsState::Nonmatching;
    };
    if tools.iter().any(|tool| {
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
    }) {
        DynamicToolsState::Matching
    } else {
        DynamicToolsState::Nonmatching
    }
}
