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

const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Cursor/OpenCode often hang on MCP during session/new. Fail the MCP-first attempt
/// quickly and retry without MCP rather than blocking a full 30s every turn.
const SESSION_SETUP_WITH_MCP_TIMEOUT: Duration = Duration::from_secs(8);
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
    // only accepts ids like `default[]`. Pin the session model through ACP after create so the
    // first prompt cannot run against a mismatched default. Launch-scoped providers already pass
    // a CLI model, so a set_session_model failure is non-fatal there.
    if matches!(
        provider,
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped
    ) {
        let session_model = prompt::configured_acp_session_model(model);
        let setup = await_model_setup(
            provider,
            SESSION_SETUP_TIMEOUT,
            connection.set_session_model(acp::SetSessionModelRequest::new(
                response.session_id.clone(),
                session_model,
            )),
        )
        .await;
        match setup {
            Ok(_) => {}
            Err(error) if provider.model_is_launch_scoped() => {
                tracing::warn!(
                    %error,
                    model,
                    "launch-scoped ACP set_session_model failed; continuing with CLI model"
                );
            }
            Err(error) => return Err(error),
        }
    }
    let session_id = response.session_id.0.to_string();
    if !crate::command_code_acp::is_command_code_model(model) {
        let include_acp_routing = prompt::should_include_acp_routing(provider, model);
        let base = prompt::provider_instructions(&params, include_acp_routing);
        if !base.is_empty() {
            instructions.borrow_mut().insert(session_id.clone(), base);
        }
    }
    Ok(json!({"thread":{"id":session_id}}))
}

async fn new_session_with_mcp(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    session_cwd: &Path,
    mcp: Vec<acp::McpServer>,
) -> Result<acp::NewSessionResponse> {
    let timeout = if mcp.is_empty() {
        SESSION_SETUP_TIMEOUT
    } else if matches!(
        provider,
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped
    ) {
        SESSION_SETUP_WITH_MCP_TIMEOUT
    } else {
        SESSION_SETUP_TIMEOUT
    };
    let mut request = acp::NewSessionRequest::new(session_cwd).mcp_servers(mcp);
    if provider != AcpProvider::Grok {
        request = request.meta(json!({ "modelId": model }).as_object().cloned());
    }
    await_setup(provider, timeout, connection.new_session(request)).await
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
    let queue_path = cache.join("launch-queue.jsonl");
    vec![acp::McpServer::Stdio(
        acp::McpServerStdio::new(LAUNCH_MCP_NAME, exe)
            .args(vec![LAUNCH_MCP_COMMAND.to_owned()])
            .env(vec![
                acp::EnvVariable::new(
                    "CLAUDEX_LAUNCH_MCP_LOG",
                    log_path.to_string_lossy().into_owned(),
                ),
                acp::EnvVariable::new(
                    "CLAUDEX_LAUNCH_QUEUE",
                    queue_path.to_string_lossy().into_owned(),
                ),
            ]),
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
// Coverage gates measure production ACP sessions; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounds_session_setup_and_reports_provider_failures() {
        let timeout = await_setup(
            AcpProvider::Configured,
            Duration::from_millis(1),
            std::future::pending::<acp::Result<()>>(),
        )
        .await
        .unwrap_err();
        assert!(timeout.to_string().contains("timed out"));
        let failed = await_setup(
            AcpProvider::Copilot,
            Duration::from_secs(1),
            std::future::ready(Err::<(), _>(acp::Error::internal_error())),
        )
        .await
        .unwrap_err();
        assert!(failed.to_string().contains("session/new failed"));
    }

    #[tokio::test]
    async fn bounds_model_setup_and_reports_provider_failures() {
        let timeout = await_model_setup(
            AcpProvider::Configured,
            Duration::from_millis(1),
            std::future::pending::<acp::Result<()>>(),
        )
        .await
        .unwrap_err();
        assert!(timeout.to_string().contains("session/set_model timed out"));

        let failed = await_model_setup(
            AcpProvider::Grok,
            Duration::from_secs(1),
            std::future::ready(Err::<(), _>(acp::Error::internal_error())),
        )
        .await
        .unwrap_err();
        assert!(failed.to_string().contains("session/set_model failed"));
    }

    #[test]
    fn accepts_only_existing_absolute_request_directories() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            request_cwd(&json!({"cwd":root.path()})),
            Some(root.path().to_owned())
        );
        assert!(request_cwd(&json!({"cwd":"relative"})).is_none());
        assert!(request_cwd(&json!({"cwd":"/definitely/missing"})).is_none());
        assert!(request_cwd(&Value::Null).is_none());
    }

    #[test]
    fn falls_back_from_invalid_system_and_request_directories() {
        let fallback = tempfile::tempdir().unwrap();
        let request = tempfile::tempdir().unwrap();
        assert_eq!(
            session_cwd(
                &json!({
                    "baseInstructions":"CWD: /definitely/missing",
                    "cwd":request.path()
                }),
                fallback.path(),
            ),
            request.path()
        );
        assert_eq!(
            session_cwd(
                &json!({
                    "baseInstructions":"CWD: relative/path",
                    "cwd":"/definitely/missing"
                }),
                fallback.path(),
            ),
            fallback.path()
        );
    }

    #[test]
    fn detects_claude_code_launch_tools_for_mcp_injection() {
        assert!(params_offer_launch_tools(&json!({
            "dynamicTools":[{"name":"Task","description":"Launch a SubAgent"}]
        })));
        assert!(params_offer_launch_tools(&json!({
            "dynamicTools":[{"name":"cc_Agent_0","description":"use `Agent`"}]
        })));
        assert!(!params_offer_launch_tools(&json!({
            "dynamicTools":[{"name":"Bash","description":"run a shell command"}]
        })));
        assert!(!params_offer_launch_tools(&json!({})));
    }

    #[test]
    fn injects_launch_mcp_when_agent_tools_are_offered() {
        let previous_home = std::env::var_os("HOME");
        let home = tempfile::tempdir().expect("launch mcp home");
        unsafe { std::env::set_var("HOME", home.path()) };
        let servers = launch_mcp_servers(&json!({
            "dynamicTools":[{"name":"Agent","description":"Launch a SubAgent"}]
        }));
        assert_eq!(servers.len(), 1);
        match &servers[0] {
            acp::McpServer::Stdio(stdio) => {
                assert_eq!(stdio.name, LAUNCH_MCP_NAME);
                assert!(stdio.args.iter().any(|arg| arg == LAUNCH_MCP_COMMAND));
                assert!(
                    stdio
                        .env
                        .iter()
                        .any(|var| var.name == "CLAUDEX_LAUNCH_MCP_LOG")
                );
                assert!(
                    stdio
                        .env
                        .iter()
                        .any(|var| var.name == "CLAUDEX_LAUNCH_QUEUE")
                );
            }
            other => panic!("expected stdio MCP, got {other:?}"),
        }
        assert!(launch_mcp_servers(&json!({})).is_empty());
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
