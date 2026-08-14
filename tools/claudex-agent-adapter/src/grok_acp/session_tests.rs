use super::*;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use super::super::client::AcpClient;
use crate::app_server::events::ThreadEventDispatcher;

fn rpc_connection(
    reply: Result<Value, &'static str>,
) -> (
    acp::ClientSideConnection,
    tokio::sync::oneshot::Receiver<Value>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
) {
    let events = Arc::new(ThreadEventDispatcher::default());
    let (client_write, server_read) = tokio::io::duplex(4096);
    let (server_write, client_read) = tokio::io::duplex(4096);
    let (connection, io) = acp::ClientSideConnection::new(
        AcpClient::new(events),
        client_write.compat_write(),
        client_read.compat(),
        |future| {
            tokio::task::spawn_local(future);
        },
    );
    let io_task = tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    let (request_tx, request_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::task::spawn_local(async move {
        let mut server_write = server_write;
        let mut reader = BufReader::new(server_read);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let id = request["id"].clone();
        request_tx.send(request).unwrap();
        let response = match reply {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => json!({
                "jsonrpc":"2.0",
                "id":id,
                "error":{"code":-32603,"message":message}
            }),
        };
        server_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
    });
    (connection, request_rx, server_task, io_task)
}

#[test]
fn session_setup_timeouts_fail_fast_without_mcp_hang() {
    assert_eq!(SESSION_SETUP_TIMEOUT, Duration::from_secs(8));
    assert_eq!(SESSION_SETUP_WITH_MCP_TIMEOUT, Duration::from_secs(2));
    assert!(SESSION_SETUP_WITH_MCP_TIMEOUT < SESSION_SETUP_TIMEOUT);
    assert!(
        SESSION_SETUP_WITH_MCP_TIMEOUT <= Duration::from_secs(3),
        "MCP-first session/new must fail fast so Nucleating is not stuck for seconds on hung MCP"
    );
    assert!(
        SESSION_SETUP_WITH_MCP_TIMEOUT <= Duration::from_secs(2),
        "post-create MCP attach must not extend second caps"
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Configured, true),
        SESSION_SETUP_TIMEOUT
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Configured, false),
        SESSION_SETUP_WITH_MCP_TIMEOUT
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::ConfiguredLaunchScoped, false),
        SESSION_SETUP_WITH_MCP_TIMEOUT
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Grok, false),
        SESSION_SETUP_WITH_MCP_TIMEOUT,
        "Grok MCP hang must fail fast like Configured ACP"
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Copilot, false),
        SESSION_SETUP_WITH_MCP_TIMEOUT,
        "Copilot MCP hang must fail fast like Configured ACP"
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Grok, true),
        SESSION_SETUP_TIMEOUT
    );
}

#[test]
fn launch_scoped_session_does_not_pin_model_after_create() {
    assert!(pins_acp_model_after_create(AcpProvider::Configured));
    assert!(!pins_acp_model_after_create(
        AcpProvider::ConfiguredLaunchScoped
    ));
    assert!(!pins_acp_model_after_create(AcpProvider::Grok));
    assert!(!pins_acp_model_after_create(AcpProvider::Copilot));
}

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

#[tokio::test(start_paused = true)]
async fn session_setup_timeout_error_includes_configured_duration() {
    let error = await_setup(
        AcpProvider::Configured,
        SESSION_SETUP_TIMEOUT,
        std::future::pending::<acp::Result<()>>(),
    )
    .await
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&format!("timed out after {SESSION_SETUP_TIMEOUT:?}")),
        "expected SESSION_SETUP_TIMEOUT in error, got {message}"
    );
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

#[tokio::test(flavor = "current_thread")]
async fn new_session_sends_non_grok_model_metadata_and_surfaces_rpc_errors() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let cwd = tempfile::tempdir().unwrap();
            let mcp = vec![acp::McpServer::Stdio(acp::McpServerStdio::new(
                "test-mcp",
                "/bin/echo",
            ))];
            let (connection, request, server, io) = rpc_connection(Ok(json!({
                "sessionId": "session-1"
            })));
            let response = new_session_with_mcp(
                AcpProvider::Copilot,
                &connection,
                "copilot-model",
                cwd.path(),
                mcp,
            )
            .await
            .unwrap();
            assert_eq!(response.session_id.0.as_ref(), "session-1");
            let request = request.await.unwrap();
            assert_eq!(request["method"], "session/new");
            assert_eq!(request["params"]["_meta"]["modelId"], "copilot-model");
            assert_eq!(request["params"]["mcpServers"].as_array().unwrap().len(), 1);
            drop(connection);
            server.await.unwrap();
            io.await.unwrap();

            let (connection, _request, server, io) = rpc_connection(Err("session unavailable"));
            let error = new_session_with_mcp(
                AcpProvider::Grok,
                &connection,
                "grok-model",
                cwd.path(),
                Vec::new(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("session/new failed"));
            drop(connection);
            server.await.unwrap();
            io.await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn launch_mcp_attach_handles_success_and_provider_errors() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let cwd = tempfile::tempdir().unwrap();
            let mcp = vec![acp::McpServer::Stdio(acp::McpServerStdio::new(
                "test-mcp",
                "/bin/echo",
            ))];
            let (connection, request, server, io) = rpc_connection(Ok(json!({})));
            attach_launch_mcp(
                AcpProvider::Configured,
                &connection,
                &acp::SessionId::new("session-1"),
                cwd.path(),
                mcp.clone(),
            )
            .await
            .unwrap();
            let request = request.await.unwrap();
            assert_eq!(request["method"], "session/load");
            assert_eq!(request["params"]["sessionId"], "session-1");
            assert_eq!(request["params"]["mcpServers"].as_array().unwrap().len(), 1);
            drop(connection);
            server.await.unwrap();
            io.await.unwrap();

            let (connection, _request, server, io) = rpc_connection(Err("MCP unavailable"));
            let error = attach_launch_mcp(
                AcpProvider::Configured,
                &connection,
                &acp::SessionId::new("session-2"),
                cwd.path(),
                mcp,
            )
            .await
            .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("launch MCP attach via session/load failed"),
                "contextual MCP attachment error was lost: {message}"
            );
            assert!(
                message.contains("session/load failed"),
                "underlying ACP method was lost: {message}"
            );
            drop(connection);
            server.await.unwrap();
            io.await.unwrap();
        })
        .await;
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
        "dynamicTools":[{"name":"TaskOutput","description":"read task output"}]
    })));
    assert!(params_offer_launch_tools(&json!({
        "dynamicTools":[{"name":"cc_Agent_0","description":"use `Agent`"}]
    })));
    assert!(params_offer_launch_tools(&json!({
        "dynamicTools":[{"name":"helper","description":"call `Task` for background work"}]
    })));
    assert!(params_offer_launch_tools(&json!({
        "dynamicTools":[{"name":"helper","description":"call `Agent` for background work"}]
    })));
    assert!(!params_offer_launch_tools(&json!({
        "dynamicTools":[{"name":"Bash","description":"run a shell command"}]
    })));
    assert!(!params_offer_launch_tools(&json!({
        "dynamicTools":"not-an-array"
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
            assert!(
                !stdio
                    .env
                    .iter()
                    .any(|var| var.name == "CLAUDEX_LAUNCH_OWNER")
            );
        }
        other => panic!("expected stdio MCP, got {other:?}"),
    }
    let scoped = launch_mcp_servers(&json!({
        "dynamicTools":[{"name":"Agent","description":"Launch a SubAgent"}],
        "claudexLaunchOwner":"session-a"
    }));
    match &scoped[0] {
        acp::McpServer::Stdio(stdio) => {
            let queue = stdio
                .env
                .iter()
                .find(|var| var.name == "CLAUDEX_LAUNCH_QUEUE")
                .expect("queue env");
            assert!(queue.value.contains("launch-queue.session-a.jsonl"));
            assert!(
                stdio
                    .env
                    .iter()
                    .any(|var| { var.name == "CLAUDEX_LAUNCH_OWNER" && var.value == "session-a" })
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

#[test]
fn launch_mcp_skips_injection_when_adapter_executable_is_unavailable() {
    let servers = launch_mcp_servers_from(
        &json!({
            "dynamicTools":[{"name":"Agent","description":"Launch a SubAgent"}]
        }),
        Err(std::io::Error::other("no exe")),
    );
    assert!(servers.is_empty());
}
