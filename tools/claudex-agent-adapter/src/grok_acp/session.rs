use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{connection::AcpProvider, prompt};

#[path = "session_mcp.rs"]
mod mcp;
use mcp::launch_mcp_servers;
#[cfg(test)]
use mcp::{launch_mcp_servers_from, params_offer_launch_tools};

pub(super) const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(8);
/// Bounded timeout for setup requests that carry MCP, including the
/// compatibility path used when resuming a persisted session. Do not raise
/// this; MCP setup must not hold session creation indefinitely.
pub(super) const SESSION_SETUP_WITH_MCP_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const LAUNCH_MCP_NAME: &str = "claudex-launch";
pub(super) const LAUNCH_MCP_COMMAND: &str = "mcp-claudex-launch";
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
    // A session-scoped configured ACP child can serve multiple routed model
    // targets. Prefer the request's model over the child startup model so the
    // ACP `session/new` metadata and subsequent `set_session_model` target the
    // session that actually owns this Claude request.
    let model = params.get("model").and_then(Value::as_str).unwrap_or(model);
    // Claude Code embeds the active child cwd in its base instructions. Keep ACP sessions scoped
    // to that request instead of leaking the adapter daemon's launch directory.
    let session_cwd = session_cwd(&params, cwd);
    // ACP agents receive MCP servers as part of session/new. A fresh session
    // has no persisted state to load, and session/load is a resume operation;
    // in particular, Grok and Cursor can race their persistence stores when a
    // newly returned ID is immediately loaded. Do not fall back to an MCP-less
    // session or to session/load when session/new carries launch MCP.
    let mcp = launch_mcp_servers(&params);
    let response = new_session_with_mcp(provider, connection, model, &session_cwd, mcp).await?;
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

#[path = "session_setup.rs"]
mod setup;
#[cfg(test)]
use setup::{attach_launch_mcp, await_setup, request_cwd, session_setup_timeout};
use setup::{await_model_setup, new_session_with_mcp, pins_acp_model_after_create, session_cwd};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "session_tests.rs"]
mod tests;
