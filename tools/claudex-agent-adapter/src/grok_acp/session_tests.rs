use super::*;
use serde_json::{Value, json};

#[test]
fn session_setup_timeouts_fail_fast_without_mcp_hang() {
    assert_eq!(SESSION_SETUP_TIMEOUT, Duration::from_secs(8));
    assert_eq!(SESSION_SETUP_WITH_MCP_TIMEOUT, Duration::from_secs(5));
    assert!(SESSION_SETUP_WITH_MCP_TIMEOUT < SESSION_SETUP_TIMEOUT);
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
        SESSION_SETUP_TIMEOUT
    );
    assert_eq!(
        session_setup_timeout(AcpProvider::Copilot, false),
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
        message.contains("timed out after 15s"),
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
