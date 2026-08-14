use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};

use super::super::connection::AcpProvider;
use super::{LAUNCH_MCP_NAME, SESSION_SETUP_TIMEOUT, SESSION_SETUP_WITH_MCP_TIMEOUT};
use crate::anthropic::subscription_request::cwd_from_system;

pub(super) fn pins_acp_model_after_create(provider: AcpProvider) -> bool {
    matches!(provider, AcpProvider::Configured)
}

pub(super) async fn new_session_with_mcp(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    session_cwd: &Path,
    mcp: Vec<acp::McpServer>,
) -> Result<acp::NewSessionResponse> {
    let has_mcp = !mcp.is_empty();
    let mcp_server_count = mcp.len();
    let mcp_server_names = launch_mcp_server_names(&mcp);
    let timeout = session_setup_timeout(provider, !has_mcp);
    let mut request = acp::NewSessionRequest::new(session_cwd).mcp_servers(mcp);
    if provider != AcpProvider::Grok {
        request = request.meta(json!({ "modelId": model }).as_object().cloned());
    }
    tracing::info!(
        provider = provider.label(),
        has_mcp,
        mcp_server_count,
        mcp_server_names = ?mcp_server_names,
        "ACP session/new started"
    );
    let started = Instant::now();
    let response = await_acp_rpc(
        provider,
        timeout,
        "session/new",
        connection.new_session(request),
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &response {
        Ok(_) => tracing::info!(
            provider = provider.label(),
            has_mcp,
            mcp_server_count,
            mcp_server_names = ?mcp_server_names,
            elapsed_ms,
            status = "ok",
            "ACP session/new completed"
        ),
        Err(error) => tracing::warn!(
            provider = provider.label(),
            has_mcp,
            mcp_server_count,
            mcp_server_names = ?mcp_server_names,
            elapsed_ms,
            status = "error",
            error_kind = acp_rpc_error_kind(error),
            "ACP session/new failed"
        ),
    }
    if has_mcp {
        response.with_context(|| {
            format!(
                "{} ACP launch MCP attachment during session/new failed; session creation aborted",
                provider.label()
            )
        })
    } else {
        response
    }
}

fn launch_mcp_server_names(mcp: &[acp::McpServer]) -> Vec<&'static str> {
    mcp.iter()
        .filter_map(|server| match server {
            acp::McpServer::Http(server) => {
                (server.name == LAUNCH_MCP_NAME).then_some(LAUNCH_MCP_NAME)
            }
            acp::McpServer::Sse(server) => {
                (server.name == LAUNCH_MCP_NAME).then_some(LAUNCH_MCP_NAME)
            }
            acp::McpServer::Stdio(server) => {
                (server.name == LAUNCH_MCP_NAME).then_some(LAUNCH_MCP_NAME)
            }
            _ => None,
        })
        .collect()
}

fn acp_rpc_error_kind(error: &anyhow::Error) -> &'static str {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<AcpRpcTimeout>().is_some())
    {
        "timeout"
    } else {
        "rpc"
    }
}

#[derive(Debug)]
struct AcpRpcTimeout {
    provider: &'static str,
    method: String,
    timeout: Duration,
}

impl fmt::Display for AcpRpcTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ACP {} timed out after {:?}",
            self.provider, self.method, self.timeout
        )
    }
}

impl std::error::Error for AcpRpcTimeout {}

/// Attach launch MCP only while resuming a persisted session. Fresh sessions
/// must pass their MCP servers to `session/new`; calling `session/load` with a
/// just-created ID races provider persistence and can silently lose the tools.
#[cfg(test)]
pub(super) async fn attach_launch_mcp(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    session_id: &acp::SessionId,
    session_cwd: &Path,
    mcp: Vec<acp::McpServer>,
) -> Result<()> {
    if mcp.is_empty() {
        return Ok(());
    }
    let request = acp::LoadSessionRequest::new(session_id.clone(), session_cwd).mcp_servers(mcp);
    await_acp_rpc(
        provider,
        SESSION_SETUP_WITH_MCP_TIMEOUT,
        "session/load",
        connection.load_session(request),
    )
    .await
    .with_context(|| {
        format!(
            "{} ACP launch MCP attach via session/load failed; session creation aborted",
            provider.label()
        )
    })?;
    tracing::info!(
        provider = provider.label(),
        "ACP launch MCP attached after session/new"
    );
    Ok(())
}

pub(super) fn session_setup_timeout(_provider: AcpProvider, mcp_empty: bool) -> Duration {
    // MCP hang must fail fast for every provider. Grok/Copilot used to wait the
    // full 8s SESSION_SETUP_TIMEOUT before retrying without MCP, stacking onto
    // Nucleating delays for every parallel SubAgent create_session.
    if mcp_empty {
        SESSION_SETUP_TIMEOUT
    } else {
        SESSION_SETUP_WITH_MCP_TIMEOUT
    }
}

pub(super) fn session_cwd(params: &Value, fallback: &Path) -> PathBuf {
    params
        .get("baseInstructions")
        .and_then(Value::as_str)
        .and_then(cwd_from_system)
        .or_else(|| request_cwd(params))
        .unwrap_or_else(|| fallback.to_owned())
}

pub(super) async fn await_model_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    await_acp_rpc(provider, timeout, "session/set_model", request).await
}

#[cfg(test)]
pub(super) async fn await_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    await_acp_rpc(provider, timeout, "session/new", request).await
}

async fn await_acp_rpc<T>(
    provider: AcpProvider,
    timeout: Duration,
    method: &str,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            anyhow!(AcpRpcTimeout {
                provider: provider.label(),
                method: method.to_owned(),
                timeout,
            })
        })?
        .map_err(|error| anyhow!("{} ACP {method} failed: {error:?}", provider.label()))
}

pub(super) fn request_cwd(params: &Value) -> Option<PathBuf> {
    params
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
}
