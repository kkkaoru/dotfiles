use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use super::super::connection::AcpProvider;
use super::{SESSION_SETUP_TIMEOUT, SESSION_SETUP_WITH_MCP_TIMEOUT};
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
    let timeout = session_setup_timeout(provider, mcp.is_empty());
    let mut request = acp::NewSessionRequest::new(session_cwd).mcp_servers(mcp);
    if provider != AcpProvider::Grok {
        request = request.meta(json!({ "modelId": model }).as_object().cloned());
    }
    await_setup(provider, timeout, connection.new_session(request)).await
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

pub(super) async fn await_setup<T>(
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

pub(super) fn request_cwd(params: &Value) -> Option<PathBuf> {
    params
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
}
