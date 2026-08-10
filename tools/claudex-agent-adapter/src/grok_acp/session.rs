use std::{
    cell::RefCell,
    collections::HashMap,
    env,
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{connection::AcpProvider, prompt};
use crate::anthropic::subscription_request::cwd_from_system;

const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(12);
/// Cursor/OpenCode often hang on MCP during session/new. Fail the MCP-first attempt
/// quickly and retry without MCP rather than blocking a full SESSION_SETUP_TIMEOUT every turn.
const SESSION_SETUP_WITH_MCP_TIMEOUT: Duration = Duration::from_secs(5);
const LAUNCH_MCP_NAME: &str = "claudex-launch";
const LAUNCH_MCP_COMMAND: &str = "mcp-claudex-launch";
pub(super) struct Task {
    pub(super) provider: AcpProvider,
    pub(super) connection: Rc<acp::ClientSideConnection>,
    pub(super) model: String,
    pub(super) cwd: PathBuf,
    pub(super) params: Value,
    pub(super) instructions: Rc<RefCell<HashMap<String, String>>>,
    pub(super) permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) response: oneshot::Sender<Result<Value>>,
}

impl Task {
    pub(super) fn spawn(self) {
        tokio::task::spawn_local(async move {
            let result = create(
                self.provider,
                &self.connection,
                &self.model,
                &self.cwd,
                self.params,
                &self.instructions,
            )
            .await;
            drop(self.permit);
            let _ = self.response.send(result);
        });
    }
}

pub(super) async fn create(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    cwd: &Path,
    params: Value,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<Value> {
    // Claude Code embeds the active child cwd in its base instructions. Keep ACP sessions scoped
    // to that request instead of leaking the adapter daemon's launch directory.
    let session_cwd = session_cwd(&params, cwd);
    // Attach a tiny MCP server that exposes Agent/Task when Claude Code supplied them.
    // ACP providers otherwise only see native tools (and Grok's spawn_subagent), so SubAgent
    // launches never become Claude Code tool_use and stay invisible in the agents panel.
    // Cursor/OpenCode have historically hung on session/new while waiting for MCP; if that
    // happens, retry once without launch MCP so the turn still starts.
    let mcp = launch_mcp_servers(&params);
    let injected_launch_mcp = !mcp.is_empty();
    let response = match new_session_with_mcp(provider, connection, model, &session_cwd, mcp).await
    {
        Ok(response) => {
            if injected_launch_mcp {
                tracing::info!(
                    provider = provider.label(),
                    "ACP session/new with launch MCP succeeded"
                );
            }
            response
        }
        Err(error) if !injected_launch_mcp => return Err(error),
        Err(error) => {
            tracing::warn!(
                %error,
                provider = provider.label(),
                "ACP session/new with launch MCP failed; retrying without MCP"
            );
            new_session_with_mcp(provider, connection, model, &session_cwd, Vec::new()).await?
        }
    };
    // OpenCode ignores modelId meta on session/new. Cursor accepts CLI `--model auto` but ACP
    // only accepts ids like `default[]`. Pin session-scoped configured ACP after create so the
    // first prompt cannot run against a mismatched default. Launch-scoped CLIs already pass
    // `--model`; a post-create set_session_model RPC only adds seconds of Nucleating delay.
    if pins_acp_model_after_create(provider) {
        let session_model = prompt::configured_acp_session_model(model);
        await_model_setup(
            provider,
            SESSION_SETUP_TIMEOUT,
            connection.set_session_model(acp::SetSessionModelRequest::new(
                response.session_id.clone(),
                session_model,
            )),
        )
        .await?;
    }
    let session_id = response.session_id.0.to_string();
    if !crate::command_code_acp::is_command_code_model(model) {
        let include_acp_routing = prompt::should_include_acp_routing(provider, model)
            && !prompt::is_acp_worker_session(&params);
        let base = prompt::provider_instructions(&params, include_acp_routing);
        if !base.is_empty() {
            instructions.borrow_mut().insert(session_id.clone(), base);
        }
    }
    Ok(json!({"thread":{"id":session_id}}))
}

fn pins_acp_model_after_create(provider: AcpProvider) -> bool {
    matches!(provider, AcpProvider::Configured)
}

async fn new_session_with_mcp(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    session_cwd: &Path,
    mcp: Vec<acp::McpServer>,
) -> Result<acp::NewSessionResponse> {
    let timeout = session_setup_timeout(provider, mcp.is_empty());
    let mut request = acp::NewSessionRequest::new(session_cwd).mcp_servers(mcp);
    if provider != AcpProvider::Grok {
        request = request.meta(json!({ "modelId": model }).as_object().cloned());
    }
    await_setup(provider, timeout, connection.new_session(request)).await
}

fn session_setup_timeout(provider: AcpProvider, mcp_empty: bool) -> Duration {
    if mcp_empty {
        SESSION_SETUP_TIMEOUT
    } else if matches!(
        provider,
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped
    ) {
        SESSION_SETUP_WITH_MCP_TIMEOUT
    } else {
        SESSION_SETUP_TIMEOUT
    }
}

fn session_cwd(params: &Value, fallback: &Path) -> PathBuf {
    params
        .get("baseInstructions")
        .and_then(Value::as_str)
        .and_then(cwd_from_system)
        .or_else(|| request_cwd(params))
        .unwrap_or_else(|| fallback.to_owned())
}

fn launch_mcp_servers(params: &Value) -> Vec<acp::McpServer> {
    if !params_offer_launch_tools(params) {
        return Vec::new();
    }
    let Ok(exe) = env::current_exe() else {
        tracing::warn!("adapter executable unavailable; ACP Agent/Task tools not injected");
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

fn params_offer_launch_tools(params: &Value) -> bool {
    params
        .get("dynamicTools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| {
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
        })
}

async fn await_model_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            anyhow!(
                "{} ACP session/set_model timed out after {:?}",
                provider.label(),
                timeout
            )
        })?
        .map_err(|error| {
            anyhow!(
                "{} ACP session/set_model failed: {error:?}",
                provider.label()
            )
        })
}

async fn await_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            anyhow!(
                "{} ACP session/new timed out after {:?}",
                provider.label(),
                timeout
            )
        })?
        .map_err(|error| anyhow!("{} ACP session/new failed: {error:?}", provider.label()))
}

fn request_cwd(params: &Value) -> Option<PathBuf> {
    params
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "session_tests.rs"]
mod tests;
