use std::{fs, time::Duration};

use serde_json::json;

use super::subscription::{
    cwd_from_system, request_effort, requested_tools, setting_at, subscription_limits_from,
    subscription_prompt, valid_effort, wait_for_subscription,
};

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

#[tokio::test]
async fn times_out_a_stalled_subscription_process() {
    let stalled = std::future::pending::<std::io::Result<std::process::Output>>();
    let error = wait_for_subscription(stalled, Duration::from_millis(1))
        .await
        .expect_err("stalled subscription must time out");
    assert!(error.to_string().contains("timed out"));
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
        ]
    );
}
