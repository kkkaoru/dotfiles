use std::{fs, path::Path, sync::Arc, time::Duration};

#[cfg(unix)]
use std::{
    os::unix::fs::PermissionsExt,
    process::{Command as StdCommand, Stdio},
};

use serde_json::json;

use super::subscription::{
    OutputMode, SubscriptionOptions, cwd_from_system, request_effort, requested_tools,
    run_subscription_model, setting_at, should_retry_subscription, subscription_command,
    subscription_limits_from, subscription_prompt, transient_retry_delay, valid_effort,
};
use crate::NONINTERACTIVE_CHILD_ENV;
use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::{
    Bridge, MessagesRequest, agent_effort::AgentEffort, subscription_request::is_compaction_request,
};
use crate::provider_config::WorkerRoute;

/// llvm-cov parallel load can stall shell fixtures past the default 5s child bound.
#[cfg(coverage_nightly)]
const SUBSCRIPTION_FIXTURE_TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(not(coverage_nightly))]
const SUBSCRIPTION_FIXTURE_TIMEOUT: Duration = Duration::from_secs(5);

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
fn subscription_children_drop_isolated_claude_config_dir() {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let command = subscription_command(
        Path::new("claude"),
        "claude-opus-5",
        &options,
        OutputMode::Json,
    );
    let envs: Vec<_> = command.as_std().get_envs().collect();
    for name in [
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_API_KEY",
        "CLAUDEX_ACTIVE",
    ] {
        assert!(
            envs.iter()
                .any(|(key, value)| *key == std::ffi::OsStr::new(name) && value.is_none()),
            "subscription child must unset {name} so OAuth login stays on ~/.claude"
        );
    }
}

#[test]
fn subscription_without_explicit_tools_does_not_disable_claude_tools() {
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
    let args = command.as_std().get_args().collect::<Vec<_>>();
    assert!(
        !args
            .iter()
            .any(|argument| argument.to_str() == Some("--tools"))
    );
    assert!(
        !args
            .iter()
            .any(|argument| argument.to_str() == Some("--allowedTools"))
    );
}

#[test]
fn explicit_empty_tools_disable_customizations_and_preserve_structured_output() {
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let schema = r#"{"additionalProperties":false,"properties":{"ok":{"type":"boolean"}},"required":["ok"],"type":"object"}"#;
    options.disable_tools = true;
    options.json_schema = Some(schema.to_owned());
    let command = subscription_command(
        Path::new("claude"),
        "claude-opus-5",
        &options,
        OutputMode::StreamJson,
    );
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert!(args.iter().any(|argument| argument == "--safe-mode"));
    assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
    assert!(args.windows(2).any(|pair| pair == ["--allowedTools", ""]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--json-schema", schema])
    );
}

#[test]
fn detects_only_the_latest_verified_compaction_instruction() {
    let request = |messages| MessagesRequest {
        model: "claude-opus-5".to_owned(),
        system: json!(null),
        messages,
        tools: Vec::new(),
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let summary = concat!(
        "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.\n",
        "Your task is to create a detailed summary of the conversation so far"
    );
    for content in [
        json!(summary),
        json!([{"type":"text", "text":summary}]),
        json!("/compact Preserve the active goal and current implementation state."),
        json!("<command-name>/compact</command-name>"),
    ] {
        assert!(is_compaction_request(&request(vec![json!({
            "role":"user", "content":content
        })])));
    }
    assert!(!is_compaction_request(&request(vec![
        json!({"role":"user", "content":summary}),
        json!({"role":"assistant", "content":"prior summary"}),
        json!({"role":"user", "content":"continue ordinary work"}),
    ])));
    assert!(!is_compaction_request(&request(vec![json!({
        "role":"user", "content":"/compaction is not a command"
    })])));
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
        "Bash".to_owned(),
    ];
    let command = subscription_command(
        Path::new("claude"),
        "claude-sonnet-5",
        &options,
        OutputMode::StreamJson,
    );
    let args = command.as_std().get_args().collect::<Vec<_>>();
    assert!(
        !args
            .iter()
            .any(|argument| argument.to_str() == Some("--settings")),
        "streaming subscription must not install a hook that rejects shell commands"
    );
    let allowed_tools = args
        .windows(2)
        .find_map(|pair| (pair[0].to_str() == Some("--allowedTools")).then_some(pair[1]))
        .expect("allowed tools")
        .to_str()
        .expect("UTF-8 allowed tools");
    assert!(allowed_tools.split(',').any(|tool| tool == "Bash"));
}

#[tokio::test]
async fn closed_subscription_capacity_is_reported() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    slots.close();

    let error = super::subscription::acquire_subscription_slot(slots, Duration::from_secs(1))
        .await
        .expect_err("closed subscription capacity must reject a new permit");

    assert_eq!(error.to_string(), "Claude subscription capacity is closed");
}

#[tokio::test(start_paused = true)]
async fn subscription_capacity_wait_times_out_when_every_slot_is_held() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let _held_slot = Arc::clone(&slots)
        .try_acquire_owned()
        .expect("hold the only subscription slot");

    let error = super::subscription::acquire_subscription_slot(slots, Duration::from_secs(1))
        .await
        .expect_err("waiting for a held subscription slot must time out");

    assert_eq!(
        error.to_string(),
        "Claude subscription capacity wait timed out"
    );
}

#[cfg(unix)]
#[test]
fn subscription_spawn_error_names_the_model() {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let mut command = subscription_command(
        Path::new("/definitely-missing-claude-subscription-fixture"),
        "claude-test",
        &options,
        OutputMode::Json,
    );

    let error = super::subscription::spawn_subscription(&mut command, "claude-test")
        .expect_err("a missing subscription program must fail to spawn");

    assert!(
        error
            .to_string()
            .contains("failed to start Claude subscription model claude-test")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn subscription_stdin_cannot_be_taken_twice() {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let mut command = subscription_command(Path::new("true"), "model", &options, OutputMode::Json);
    let mut child = super::subscription::spawn_subscription(&mut command, "model")
        .expect("spawn existing subscription process fixture");
    let stdin = super::subscription::take_subscription_stdin(&mut child)
        .expect("take the subscription stdin once");

    let error = super::subscription::take_subscription_stdin(&mut child)
        .expect_err("subscription stdin cannot be taken twice");
    assert!(error.to_string().contains("stdin is unavailable"));

    drop(stdin);
    assert!(child.wait().await.expect("reap process fixture").success());
}

#[cfg(unix)]
#[tokio::test]
async fn closed_subscription_stdin_rejects_a_prompt_write() {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let mut command = subscription_command(Path::new("true"), "model", &options, OutputMode::Json);
    let mut child = super::subscription::spawn_subscription(&mut command, "model")
        .expect("spawn existing subscription process fixture");
    let stdin = super::subscription::take_subscription_stdin(&mut child)
        .expect("take the subscription stdin");
    assert!(
        child
            .wait()
            .await
            .expect("wait for process fixture")
            .success()
    );

    let error = super::subscription::write_subscription_prompt(stdin, "prompt")
        .await
        .expect_err("a reaped subscription child must reject prompt writes");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("broken pipe")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn subscription_fallback_child_executes_the_bash_git_and_gh_probe() {
    let directory = tempfile::tempdir().expect("create subscription command fixture directory");
    let program = directory.path().join("subscription-command-fixture.sh");
    fs::write(
        &program,
        r#"#!/bin/sh
set -eu
tools=""
allowed_tools=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --tools) tools="$2"; shift 2 ;;
        --allowedTools) allowed_tools="$2"; shift 2 ;;
        *) shift ;;
    esac
done
case ",$tools," in *,Bash,*) ;; *) exit 21 ;; esac
case ",$allowed_tools," in *,Bash,*) ;; *) exit 22 ;; esac
cat >/dev/null
git --version >/dev/null
gh --version >/dev/null
printf '%s\n' '{"type":"result","subtype":"success","result":"SUBSCRIPTION_COMMAND_PROBE_OK"}'
"#,
    )
    .expect("write subscription command fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make subscription command fixture executable");
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        SUBSCRIPTION_FIXTURE_TIMEOUT,
    );
    options.tools = vec!["Bash".to_owned()];

    let result = run_subscription_model(&program, "claude-sonnet-5", "command probe", options)
        .await
        .expect("subscription child must receive Bash and execute git and gh");

    assert_eq!(result, "SUBSCRIPTION_COMMAND_PROBE_OK");
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
        Duration::from_secs(1),
    );

    let error = run_subscription_model(&program, "model", "prompt", options)
        .await
        .expect_err("stalled subscription must time out");

    assert!(error.to_string().contains("timed out"));
    let failure = super::subscription::subscription_failure(&error)
        .expect("subscription timeout must be typed");
    assert_eq!(failure.status_hint(), 424);
    assert!(!failure.is_internal_retryable());
    assert!(!failure.is_outer_retryable());
    assert!(!should_retry_subscription(&error));
    assert_eq!(
        super::error::http_status(axum::http::StatusCode::BAD_GATEWAY, &error),
        axum::http::StatusCode::FAILED_DEPENDENCY
    );
    assert_eq!(
        super::error::error_type(&error),
        super::error::NON_RETRYABLE_ERROR_TYPE
    );
}

#[tokio::test]
async fn does_not_retry_a_typed_subscription_timeout() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);

    let error = super::subscription::with_transient_retries("claude-test", || {
        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::future::ready(Err::<(), _>(super::subscription::failure::timeout_failure(
            "claude-test",
            Duration::from_secs(5),
        )))
    })
    .await
    .expect_err("typed timeout must remain terminal");

    assert!(error.to_string().contains("timed out"));
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn retries_an_empty_local_subscription_exit_exactly_once() {
    let directory = tempfile::tempdir().expect("create local retry fixture directory");
    let attempts = directory.path().join("attempts");
    let program = directory.path().join("local-retry-fixture.sh");
    fs::write(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
cat >/dev/null
printf 'attempt\n' >> '{attempts}'
if [ "$(wc -l < '{attempts}')" -eq 1 ]; then
    exit 1
fi
printf '%s\n' '{{"subtype":"success","is_error":false,"result":"RETRIED_OK"}}'
"#,
            attempts = attempts.display(),
        ),
    )
    .expect("write local retry fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make local retry fixture executable");
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        SUBSCRIPTION_FIXTURE_TIMEOUT,
    );

    let result = run_subscription_model(&program, "claude-test", "prompt", options)
        .await
        .expect("empty local exit should retry once");

    assert_eq!(result, "RETRIED_OK");
    assert_eq!(
        fs::read_to_string(attempts)
            .expect("attempt log")
            .lines()
            .count(),
        2
    );
}

#[cfg(unix)]
#[tokio::test]
async fn does_not_retry_a_structured_upstream_502_internally() {
    let directory = tempfile::tempdir().expect("create retry fixture directory");
    let attempts = directory.path().join("attempts");
    let program = directory.path().join("retry-fixture.sh");
    let attempts_path = attempts.display();
    fs::write(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
cat >/dev/null
printf 'attempt\n' >> '{attempts_path}'
printf '%s\n' '{{"subtype":"error","is_error":true,"result":"502 Bad Gateway"}}'
"#
        ),
    )
    .expect("write retry fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make retry fixture executable");

    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        SUBSCRIPTION_FIXTURE_TIMEOUT,
    );
    let error = run_subscription_model(&program, "claude-haiku-4-5", "prompt", options)
        .await
        .expect_err("upstream 502 must be delegated to the outer retry policy");

    assert!(error.to_string().contains("502 Bad Gateway"));
    assert_eq!(
        fs::read_to_string(&attempts)
            .expect("attempt log")
            .lines()
            .count(),
        1
    );
    assert!(!should_retry_subscription(&error));
    assert!(!should_retry_subscription(&anyhow::anyhow!(
        "502 Bad Gateway"
    )));
    assert!(!should_retry_subscription(&anyhow::anyhow!(
        "Claude subscription model claude-haiku-4-5 exited with exit status: 1: fixture failure"
    )));
    assert!(!should_retry_subscription(&anyhow::anyhow!(
        "502 Bad Gateway; subscription stream already emitted frames"
    )));
    assert_eq!(transient_retry_delay(1), Duration::from_millis(100));
}

#[cfg(unix)]
#[tokio::test]
async fn nonzero_subscription_exit_prefers_sanitized_stdout_json() {
    let directory = tempfile::tempdir().expect("create failure fixture directory");
    let program = directory.path().join("stdout-failure-fixture.sh");
    fs::write(
        &program,
        r#"#!/bin/sh
set -eu
cat >/dev/null
printf '%s\n' '{"subtype":"error","is_error":true,"result":"Authentication failed api_key=fixture-secret"}'
printf '%s\n' 'less useful stderr' >&2
exit 1
"#,
    )
    .expect("write failure fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make failure fixture executable");
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        SUBSCRIPTION_FIXTURE_TIMEOUT,
    );

    let error = run_subscription_model(&program, "claude-test", "prompt", options)
        .await
        .expect_err("nonzero subscription process must fail");
    let message = error.to_string();
    assert!(message.contains("Authentication failed"));
    assert!(message.contains("trace="));
    assert!(!message.contains("less useful stderr"));
    assert!(!message.contains("fixture-secret"));
    assert!(!should_retry_subscription(&error));
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

#[tokio::test]
async fn closing_an_activity_that_never_opened_sends_no_frame() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let mut activity = super::subscription_activity::SubscriptionActivity::default();
    assert!(!activity.is_open());
    activity
        .close(&sender)
        .await
        .expect("close on a never-opened activity is a no-op");
    assert!(!activity.is_open());
    drop(sender);
    assert!(
        receiver.recv().await.is_none(),
        "closing an unopened activity must not emit a signature_delta or block_stop frame"
    );
}

#[tokio::test]
async fn start_status_is_a_noop_when_empty_or_already_open() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
    let mut activity = super::subscription_activity::SubscriptionActivity::default();
    let mut next_index = 0;
    activity
        .start_status(&sender, "", &mut next_index)
        .await
        .expect("empty status is ignored");
    assert_eq!(next_index, 0);
    assert!(!activity.is_open());

    activity
        .start_status(&sender, "working", &mut next_index)
        .await
        .expect("open status");
    assert!(activity.is_open());
    assert_eq!(next_index, 1);
    activity
        .start_status(&sender, "again", &mut next_index)
        .await
        .expect("second open is ignored");
    assert_eq!(next_index, 1);
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
fn resolves_effort_from_the_latest_claude_settings_file() {
    let directory = tempfile::tempdir().expect("create dynamic effort settings");
    let settings_path = directory.path().join("settings.json");
    fs::write(&settings_path, r#"{"effortLevel":"high"}"#).expect("write high settings");
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned())
        .with_settings_path(&settings_path);
    let request = MessagesRequest {
        model: "claude-sonnet-5".to_owned(),
        system: json!(null),
        messages: vec![],
        tools: vec![json!({"name": "claude_collaborator"})],
        stream: false,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::Unmatched),
        Some("high".to_owned())
    );
    fs::write(&settings_path, r#"{"effortLevel":"xhigh"}"#).expect("update xhigh settings");
    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::Unmatched),
        Some("xhigh".to_owned())
    );
    fs::write(&settings_path, r#"{"effortLevel":"invalid"}"#).expect("write invalid settings");
    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::Unmatched),
        None
    );
    fs::write(&settings_path, r#"{"effortLevel":"high"}"#).expect("restore high settings");
    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::ConfiguredDefault),
        Some("high".to_owned())
    );
    fs::write(&settings_path, r#"{"effortLevel":"invalid"}"#).expect("write invalid default");
    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::ConfiguredDefault),
        None
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

#[test]
fn subagent_subscription_omits_main_only_advisor_tool() {
    let request = MessagesRequest {
        model: "gpt-5.3-codex-spark".to_owned(),
        system: json!("cc_is_subagent=true"),
        messages: vec![],
        tools: vec![
            json!({"name":"Read"}),
            json!({"name":"advisor"}),
            json!({"name":"Bash"}),
        ],
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    assert_eq!(
        super::subscription::requested_tools_for_request(&request, true),
        ["Read", "Bash"]
    );
}

#[test]
fn subagent_subscription_options_select_a_short_initial_activity_delay() {
    use crate::anthropic::subscription_stream::{
        ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    };

    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let request = MessagesRequest {
        model: "claude-sonnet-5".to_owned(),
        system: json!(null),
        messages: vec![],
        tools: vec![],
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    let main_options = bridge.subscription_options(&request, None, false, false);
    assert_eq!(main_options.initial_activity_delay, INITIAL_ACTIVITY_DELAY);

    let subagent_options = bridge.subscription_options(&request, None, true, false);
    assert_eq!(
        subagent_options.initial_activity_delay,
        SUBAGENT_INITIAL_ACTIVITY_DELAY
    );
    assert!(subagent_options.initial_activity_delay < main_options.initial_activity_delay);
    assert_eq!(
        main_options.activity_keepalive_interval,
        ACTIVITY_KEEPALIVE_INTERVAL
    );
    assert_eq!(
        subagent_options.activity_keepalive_interval,
        ACTIVITY_KEEPALIVE_INTERVAL
    );
}

#[test]
fn search_worker_does_not_receive_unrequested_native_web_tools() {
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

    assert!(super::subscription::requested_tools_for_request(&request, true).is_empty());
}

#[test]
fn resumed_subscription_request_does_not_infer_tools_from_history() {
    let request = MessagesRequest {
        model: "claude-opus-5".to_owned(),
        system: json!("resumed main session"),
        messages: vec![json!({
            "role": "assistant",
            "content": [{"type": "tool_use", "name": "Bash", "input": {}}]
        })],
        tools: vec![],
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    assert!(super::subscription::requested_tools_for_request(&request, false).is_empty());
}

#[test]
fn configured_worker_effort_replaces_an_unsupported_explicit_effort() {
    let mut bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    bridge
        .model_catalog
        .set_worker_routes(vec![WorkerRoute::new(
            "claudex-gpt-spark".to_owned(),
            "gpt-5.3-codex-spark".to_owned(),
            "xhigh".to_owned(),
        )])
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
    assert_eq!(
        bridge.resolve_request_effort(&request, AgentEffort::ConfiguredDefault),
        Some("xhigh".to_owned()),
        "ConfiguredDefault must use the worker route effort when settings are absent"
    );
}

#[test]
fn native_grok_route_effort_overrides_explicit_and_unmatched_request_effort() {
    let mut route = BackendRoute::new("grok-4.6", BackendKind::GrokAcp);
    route.effort = Some("medium".to_owned());
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[route]), "main".to_owned());
    let mut request = MessagesRequest {
        model: "grok-4.6".to_owned(),
        system: json!(null),
        messages: vec![],
        tools: vec![],
        stream: false,
        output_config: json!({"effort":"low"}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };

    for requested in ["low", "max"] {
        request.output_config = json!({"effort":requested});
        assert_eq!(
            bridge.resolve_request_effort(&request, AgentEffort::Unmatched),
            Some("medium".to_owned())
        );
        assert_eq!(
            bridge.resolve_request_effort(&request, AgentEffort::Explicit(requested.to_owned())),
            Some("medium".to_owned())
        );
    }

    let mut configured =
        BackendRoute::new("opencode-go/deepseek-v4-flash", BackendKind::ConfiguredAcp);
    configured.effort = Some("max".to_owned());
    let configured_bridge =
        Bridge::new_with_backend(AgentBackend::spawn_routes(&[configured]), "main".to_owned());
    request.model = "opencode-go/deepseek-v4-flash".to_owned();
    request.output_config = json!({"effort":"low"});
    assert_eq!(
        configured_bridge.resolve_request_effort(&request, AgentEffort::Unmatched),
        Some("low".to_owned())
    );
}
