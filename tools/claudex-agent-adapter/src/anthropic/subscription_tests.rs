use std::{fs, path::Path, sync::Arc, time::Duration};

#[cfg(unix)]
use std::{
    os::unix::fs::PermissionsExt,
    process::{Command as StdCommand, Stdio},
};

use serde_json::json;

use super::subscription::{
    OutputMode, SubscriptionOptions, cwd_from_system, request_effort, requested_tools,
    run_subscription_model, setting_at, subscription_command, subscription_limits_from,
    subscription_prompt, valid_effort,
};
use crate::NONINTERACTIVE_CHILD_ENV;
use crate::agent_backend::AgentBackend;
use crate::anthropic::{Bridge, MessagesRequest, agent_effort::AgentEffort};
use crate::provider_config::WorkerRoute;

#[test]
fn subscription_children_identify_as_noninteractive() {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let command = subscription_command(
        Path::new("claude"),
        "claude-sonnet-5",
        &options,
        OutputMode::Json,
    );
    let value = command
        .as_std()
        .get_envs()
        .find_map(|(name, value)| (name == NONINTERACTIVE_CHILD_ENV).then_some(value))
        .flatten();
    assert_eq!(value, Some(std::ffi::OsStr::new("1")));
}

#[test]
fn streaming_subscription_bridges_native_web_tools_to_the_outer_session() {
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    options.tools = vec![
        "WebSearch".to_owned(),
        "WebFetch".to_owned(),
        "Agent".to_owned(),
    ];
    let command = subscription_command(
        Path::new("claude"),
        "claude-sonnet-5",
        &options,
        OutputMode::StreamJson,
    );
    let args = command.as_std().get_args().collect::<Vec<_>>();
    let settings = args
        .windows(2)
        .find_map(|pair| (pair[0].to_str() == Some("--settings")).then_some(pair[1]))
        .expect("stream settings")
        .to_str()
        .expect("UTF-8 settings");
    let value: serde_json::Value = serde_json::from_str(settings).expect("settings JSON");
    assert_eq!(
        value
            .pointer("/hooks/PreToolUse/0/matcher")
            .and_then(serde_json::Value::as_str),
        Some("^(?!WebSearch$|WebFetch$).*")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn terminating_a_subscription_reaps_its_entire_process_group() {
    let directory = tempfile::tempdir().expect("create process fixture directory");
    let program = directory.path().join("subscription-fixture.sh");
    fs::write(&program, "#!/bin/sh\nsleep 30 &\nwait\n").expect("write process fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make process fixture executable");
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let mut command = subscription_command(&program, "model", &options, OutputMode::Json);
    let mut child = super::subscription::spawn_subscription(&mut command, "model")
        .expect("spawn isolated subscription process group");
    let process_group = child.id().expect("child process group ID");
    tokio::time::sleep(Duration::from_millis(20)).await;

    super::subscription::terminate_subscription(&mut child)
        .await
        .expect("terminate and reap subscription process group");

    assert!(child.try_wait().expect("inspect child").is_some());
    assert!(
        !StdCommand::new("kill")
            .args(["-0", &format!("-{process_group}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("inspect process group")
            .success()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn subscription_timeout_terminates_and_reaps_the_model_process() {
    let directory = tempfile::tempdir().expect("create timeout fixture directory");
    let program = directory.path().join("timeout-fixture.sh");
    fs::write(&program, "#!/bin/sh\nsleep 30 &\nwait\n").expect("write timeout fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make timeout fixture executable");
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_millis(100),
    );

    let error = run_subscription_model(&program, "model", "prompt", options)
        .await
        .expect_err("stalled subscription must time out");

    assert!(error.to_string().contains("timed out"));
}

#[test]
fn subscription_limits_use_documented_defaults() {
    let limits = subscription_limits_from(|_| None);
    assert_eq!(limits.max_processes, 20);
    assert_eq!(limits.timeout, Duration::from_secs(120 * 60));
}

#[test]
fn subscription_limits_accept_independent_environment_overrides() {
    let limits = subscription_limits_from(|name| match name {
        "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES" => Some("7".to_owned()),
        "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES" => Some("45".to_owned()),
        _ => None,
    });
    assert_eq!(limits.max_processes, 7);
    assert_eq!(limits.timeout, Duration::from_secs(45 * 60));
}

#[test]
fn subscription_limits_reject_zero_invalid_and_overflowing_values() {
    let limits = subscription_limits_from(|name| match name {
        "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES" => Some("0".to_owned()),
        "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES" => Some(u64::MAX.to_string()),
        _ => None,
    });
    assert_eq!(limits.max_processes, 20);
    assert_eq!(limits.timeout, Duration::from_secs(120 * 60));

    let excessive = subscription_limits_from(|name| {
        (name == "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES").then(|| usize::MAX.to_string())
    });
    assert_eq!(excessive.max_processes, 20);
}

#[test]
fn validates_direct_subscription_limits() {
    assert!(super::subscription::SubscriptionLimits::new(0, 1).is_err());
    assert!(
        super::subscription::SubscriptionLimits::new(tokio::sync::Semaphore::MAX_PERMITS + 1, 1)
            .is_err()
    );
    assert!(super::subscription::SubscriptionLimits::new(1, 0).is_err());
    assert!(super::subscription::SubscriptionLimits::new(1, u64::MAX).is_err());
    let valid = super::subscription::SubscriptionLimits::new(2, 3).expect("valid limits");
    assert_eq!(valid.max_processes, 2);
    assert_eq!(valid.timeout, Duration::from_secs(180));
}

#[tokio::test]
async fn emits_subscription_activity_for_text_and_thinking_paths() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let mut activity = super::subscription_activity::SubscriptionActivity::default();
    let mut next_index = 0;
    activity
        .keepalive(&sender, Some(3), &mut next_index)
        .await
        .expect("text heartbeat");
    activity
        .keepalive(&sender, None, &mut next_index)
        .await
        .expect("thinking status");
    activity
        .keepalive(&sender, None, &mut next_index)
        .await
        .expect("thinking heartbeat");
    activity.close(&sender).await.expect("close activity");
    assert!(receiver.recv().await.is_some());
}

#[test]
fn builds_subscription_prompts() {
    assert!(subscription_prompt("advisor", &json!({}), &[]).contains("rigorous advisor"));
    assert!(
        subscription_prompt("claude_collaborator", &json!({"task":"check"}), &[]).contains("check")
    );
    assert!(
        subscription_prompt("claude_collaborator", &json!({}), &[])
            .contains("suggest the next step")
    );
}

#[test]
fn reads_effort_dynamically_from_claude_settings() {
    let directory = tempfile::tempdir().expect("create settings directory");
    let settings_path = directory.path().join("settings.json");
    fs::write(&settings_path, r#"{"effortLevel":"high"}"#).expect("write settings");
    assert_eq!(
        setting_at(&settings_path, "effortLevel").as_deref(),
        Some("high")
    );
    fs::write(&settings_path, r#"{"effortLevel":"xhigh"}"#).expect("update settings");
    assert_eq!(
        setting_at(&settings_path, "effortLevel").as_deref(),
        Some("xhigh")
    );
}

#[test]
fn validates_request_and_settings_effort_values() {
    assert_eq!(request_effort(&json!({"effort":"low"})), Some("low"));
    assert_eq!(request_effort(&json!({"effort":"invalid"})), None);
    assert_eq!(request_effort(&json!({})), None);
    assert!(valid_effort("max"));
    assert!(!valid_effort("minimal"));
}

#[test]
fn selects_subscription_workspace_and_outer_tools() {
    let directory = tempfile::tempdir().expect("create workspace");
    let workspace = directory
        .path()
        .canonicalize()
        .expect("canonical workspace");
    for label in ["CWD", "Working directory", "Primary working directory"] {
        let system = format!("<env>\n{label}: {}\n</env>", workspace.display());
        assert_eq!(
            cwd_from_system(&system).as_deref(),
            Some(workspace.as_path())
        );
    }
    assert!(cwd_from_system("CWD: relative/path").is_none());
    assert!(cwd_from_system("CWD: /path/that/does/not/exist").is_none());

    let requested = [
        json!({"name":"Read"}),
        json!({"name":"mcp__server__tool"}),
        json!({"name":"custom_tool"}),
        json!({"name":""}),
        json!({"name":"Read"}),
        json!({"name":"Bash"}),
        json!({"name":"TaskCreate"}),
        json!({"name":"TaskGet"}),
        json!({"name":"TaskList"}),
        json!({"name":"TaskUpdate"}),
        json!({"name":"ToolSearch"}),
        json!({"name":"CronCreate"}),
        json!({"name":"CronDelete"}),
        json!({"name":"CronList"}),
    ];
    let tools = requested_tools(&requested, false);
    assert_eq!(
        tools,
        [
            "Read",
            "mcp__server__tool",
            "custom_tool",
            "Bash",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskUpdate",
            "ToolSearch",
            "CronCreate",
            "CronDelete",
            "CronList",
            "WebSearch",
            "WebFetch",
        ]
    );
    assert_eq!(
        requested_tools(&requested, true),
        [
            "Read",
            "mcp__server__tool",
            "custom_tool",
            "Bash",
            "ToolSearch",
            "CronCreate",
            "CronDelete",
            "CronList",
            "WebSearch",
            "WebFetch",
        ]
    );
}

#[test]
fn search_worker_receives_native_web_tools_even_when_child_tools_are_empty() {
    let request = MessagesRequest {
        model: "claude-haiku-4-5".to_owned(),
        system: json!("name: claudex-haiku-search\ntools: WebSearch,WebFetch"),
        messages: vec![],
        tools: vec![],
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    assert_eq!(
        super::subscription::requested_tools_for_request(&request, true),
        ["WebSearch", "WebFetch"]
    );
}

#[test]
fn configured_worker_effort_replaces_an_unsupported_explicit_effort() {
    let mut bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    bridge
        .model_catalog
        .set_worker_routes(vec![WorkerRoute {
            agent: "claudex-gpt-spark".to_owned(),
            model: "gpt-5.3-codex-spark".to_owned(),
            effort: "xhigh".to_owned(),
        }])
        .expect("worker route");
    let request = MessagesRequest {
        model: "gpt-5.3-codex-spark".to_owned(),
        system: json!(null),
        messages: vec![],
        tools: vec![],
        stream: false,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::Explicit("max".to_owned())),
        Some("xhigh".to_owned())
    );
}
