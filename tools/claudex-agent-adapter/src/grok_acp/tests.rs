use std::sync::{Arc, atomic::AtomicBool};

use agent_client_protocol::{self as acp, Client as _};
use serde_json::{json, value::RawValue};

use super::{
    COMMAND_QUEUE_CAPACITY, DriverCommand, DriverThread, GrokAcp, PreparedTurn,
    SESSION_QUEUE_CAPACITY, TURN_QUEUE_CAPACITY,
    client::AcpClient,
    connection::AcpProvider,
    driver::{StartTurnRequest, drive_start_turns, schedule_start_turn},
    prompt, queue,
    turns::{ActiveTurns, InvalidatedSessions, cancel_turn, drive_turn_tasks, queue_turn},
    updates,
};
use crate::app_server::events::ThreadEventDispatcher;

#[tokio::test]
async fn terminates_the_entire_provider_process_group() {
    use std::{os::unix::process::CommandExt as _, process::Stdio, time::Duration};

    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", "sleep 60 & wait"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn().unwrap();
    let process_group = child.id().unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    super::connection::terminate_process_group(process_group);
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("provider process group did not terminate")
        .unwrap();
    let group_exists = std::process::Command::new("kill")
        .args(["-0", &format!("-{process_group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!status.success());
    assert!(!group_exists);
}

#[tokio::test]
#[allow(clippy::excessive_nesting)]
async fn provider_child_drop_reaps_a_term_resistant_descendant_group() {
    use std::{os::unix::process::CommandExt as _, process::Stdio, time::Duration};

    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", "trap '' TERM; sleep 60 & wait"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let child = command.spawn().unwrap();
    let process_group = child.id().unwrap();
    let provider = super::connection::ProviderChild::new(child, process_group);
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(provider);

    for _ in 0..100 {
        let group_exists = std::process::Command::new("kill")
            .args(["-0", &format!("-{process_group}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        if !group_exists {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("provider child descendant group {process_group} survived Drop");
}

#[test]
fn detects_opencode_programs_and_injects_anti_nesting_runtime_config() {
    use std::ffi::OsString;

    assert!(super::connection::is_opencode_program(&OsString::from(
        "opencode"
    )));
    assert!(super::connection::is_opencode_program(&OsString::from(
        "/opt/homebrew/bin/opencode"
    )));
    assert!(super::connection::is_opencode_program(&OsString::from(
        "opencode-dev"
    )));
    assert!(!super::connection::is_opencode_program(&OsString::from(
        "cursor-agent"
    )));
    assert!(!super::connection::is_opencode_program(&OsString::from(
        "/usr/bin/env"
    )));
    let config = super::connection::opencode_acp_runtime_config();
    assert!(config.contains(r#""subagent_depth":0"#));
    assert!(config.contains(r#""task":"deny""#));
}

#[test]
fn maps_cursor_auto_to_the_acp_default_model_id() {
    assert_eq!(prompt::configured_acp_session_model("auto"), "default[]");
    assert_eq!(prompt::configured_acp_session_model("default"), "default[]");
    assert_eq!(
        prompt::configured_acp_session_model("opencode-go/gpt-5.6-luna"),
        "opencode-go/gpt-5.6-luna"
    );
    assert_eq!(
        prompt::configured_acp_session_model("default[]"),
        "default[]"
    );
    assert_eq!(
        prompt::configured_acp_session_model("cline-pass/deepseek-v4-flash"),
        "deepseek/deepseek-v4-flash"
    );
}

#[test]
fn identifies_each_acp_provider_and_its_model_scope() {
    assert_eq!(AcpProvider::Grok.label(), "Grok");
    assert_eq!(AcpProvider::Grok.driver_name(), "claudex-grok-acp");
    assert!(AcpProvider::Grok.model_is_launch_scoped());
    assert!(!AcpProvider::Grok.is_session_scoped_configured());

    assert_eq!(AcpProvider::Copilot.label(), "Copilot");
    assert_eq!(AcpProvider::Copilot.driver_name(), "claudex-copilot-acp");
    assert!(!AcpProvider::Copilot.model_is_launch_scoped());
    assert!(!AcpProvider::Copilot.is_session_scoped_configured());

    assert_eq!(AcpProvider::Configured.label(), "Configured");
    assert_eq!(
        AcpProvider::Configured.driver_name(),
        "claudex-configured-acp"
    );
    assert!(!AcpProvider::Configured.model_is_launch_scoped());
    assert!(AcpProvider::Configured.is_session_scoped_configured());

    assert_eq!(
        AcpProvider::ConfiguredLaunchScoped.label(),
        "ConfiguredLaunch"
    );
    assert_eq!(
        AcpProvider::ConfiguredLaunchScoped.driver_name(),
        "claudex-configured-acp"
    );
    assert!(AcpProvider::ConfiguredLaunchScoped.model_is_launch_scoped());
    assert!(!AcpProvider::ConfiguredLaunchScoped.is_session_scoped_configured());
}

#[test]
fn configured_acp_defaults_to_parallel_session_slots_when_max_concurrency_omitted() {
    assert_eq!(
        SESSION_QUEUE_CAPACITY, 1,
        "legacy serial constant remains for in-process test doubles"
    );
    assert_eq!(
        super::DEFAULT_CONFIGURED_MAX_CONCURRENCY,
        3,
        "configured ACP must not collapse to a single session when providers omit maxConcurrency"
    );
    assert_eq!(
        super::spawn::session_create_capacity(AcpProvider::Grok, None),
        TURN_QUEUE_CAPACITY,
        "Grok session/new permits must match default turn concurrency"
    );
    assert_eq!(
        super::spawn::session_create_capacity(AcpProvider::Copilot, None),
        TURN_QUEUE_CAPACITY,
        "Copilot session/new permits must match default turn concurrency"
    );
    assert_eq!(
        super::spawn::session_create_capacity(AcpProvider::Grok, Some(3)),
        3,
        "Grok/Copilot session/new must honor maxConcurrency for parallel SubAgents"
    );
    assert_eq!(
        super::spawn::session_create_capacity(AcpProvider::Copilot, Some(4)),
        4
    );
}

#[test]
fn converts_backend_prompts_and_effort() {
    assert_eq!(SESSION_QUEUE_CAPACITY, 1);
    assert_eq!(prompt::input_text(&json!("hello")), "hello");
    assert_eq!(
        prompt::input_text(&json!([{"type":"text","text":"one"},{"content":"two"}])),
        "one\ntwo"
    );
    assert_eq!(prompt::copilot_effort("mid"), Some("medium"));
    assert_eq!(prompt::copilot_effort("xhigh"), Some("xhigh"));
    assert_eq!(prompt::copilot_effort("max"), Some("max"));
    assert_eq!(prompt::copilot_effort("invalid"), None);
    assert_eq!(prompt::input_text(&serde_json::Value::Null), "");
    assert_eq!(
        prompt::input_text(&json!({"key":"value"})),
        r#"{"key":"value"}"#
    );
}

#[test]
fn removes_codex_only_bridge_instructions() {
    let params = json!({
        "baseInstructions":"project rules\n\nbackend-only",
        "developerInstructions":"backend-only"
    });
    assert!(prompt::provider_instructions(&params, true).starts_with("project rules\n\n"));
    assert!(prompt::provider_instructions(&params, true).contains("selected_workers"));
    assert!(prompt::provider_instructions(&params, true).contains("spawn_subagent"));
    assert!(prompt::provider_instructions(&params, true).contains("provider-native Task"));
    assert!(!prompt::provider_instructions(&json!({}), true).contains("claudex-xhigh"));
    assert_eq!(
        prompt::provider_instructions(&params, false),
        "project rules"
    );
}

#[test]
fn command_code_models_skip_acp_routing_prefix() {
    assert!(prompt::should_include_acp_routing(
        AcpProvider::Grok,
        "grok-4.5"
    ));
    assert!(prompt::should_include_acp_routing(
        AcpProvider::Configured,
        "auto"
    ));
    assert!(!prompt::should_include_acp_routing(
        AcpProvider::Configured,
        "meta/muse-spark-1.2-contributor"
    ));
    assert!(!prompt::should_include_acp_routing(
        AcpProvider::Copilot,
        "grok-4.5"
    ));
    assert!(prompt::is_acp_worker_session(&json!({
        "claudexAcpRole": "worker",
        "developerInstructions": "You are a provider-native ACP worker. Complete the task."
    })));
    assert!(prompt::is_acp_worker_session(&json!({
        "developerInstructions": "You are a provider-native ACP worker."
    })));
    assert!(!prompt::is_acp_worker_session(&json!({
        "claudexAcpRole": "orchestrator",
        "developerInstructions": "Claudex provider-native ACP mode is active."
    })));
    let worker_params = json!({
        "claudexAcpRole": "worker",
        "baseInstructions": "project rules",
        "developerInstructions": "You are a provider-native ACP worker."
    });
    assert_eq!(
        prompt::provider_instructions(&worker_params, false),
        "project rules"
    );
    assert!(!prompt::provider_instructions(&worker_params, false).contains("emit a short status"));
}

#[tokio::test]
async fn client_inherent_handlers_cover_permissions_and_notifications() {
    let events = Arc::new(ThreadEventDispatcher::default());
    let receiver = events.subscribe("session");
    let client = AcpClient::new(events);
    let request = permission_request(vec![acp::PermissionOption::new(
        "reject",
        "Reject",
        acp::PermissionOptionKind::RejectOnce,
    )]);
    let selected = client.request_permission(request).await.unwrap();
    assert_eq!(
        serde_json::to_value(selected).unwrap()["outcome"]["optionId"],
        json!("reject")
    );
    let cancelled = client
        .request_permission(permission_request(vec![]))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(cancelled).unwrap()["outcome"]["outcome"],
        json!("cancelled")
    );
    let allow_once = client
        .request_permission(permission_request(vec![
            acp::PermissionOption::new(
                "reject-first",
                "Reject",
                acp::PermissionOptionKind::RejectOnce,
            ),
            acp::PermissionOption::new("allow", "Allow", acp::PermissionOptionKind::AllowOnce),
        ]))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(allow_once).unwrap()["outcome"]["optionId"],
        json!("allow")
    );

    client
        .session_notification(acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new("visible"),
            ))),
        ))
        .await
        .unwrap();
    assert_eq!(receiver.recv().await.unwrap()["params"]["delta"], "visible");
    let raw = RawValue::from_string("{}".to_owned()).unwrap();
    client
        .ext_notification(acp::ExtNotification::new("unrelated", Arc::from(raw)))
        .await
        .unwrap();
}

#[tokio::test]
async fn reports_a_closed_driver_for_each_command_response_type() {
    let agent = GrokAcp::stopped_for_test();

    assert!(agent.create_session(json!({})).await.is_err());
    assert!(agent.start_turn(json!({})).await.is_err());
    assert!(
        agent.cancel_turn("session").await.is_ok(),
        "dead-driver cancel must be idempotent so leftover SubAgent cards can settle"
    );
}

async fn drop_cancel_turn_response(receiver: &mut tokio::sync::mpsc::Receiver<DriverCommand>) {
    let Some(DriverCommand::CancelTurn { response, .. }) = receiver.recv().await else {
        return;
    };
    drop(response);
}

#[tokio::test]
async fn cancel_turn_settles_when_the_driver_drops_its_response() {
    let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
    let agent = GrokAcp {
        provider: AcpProvider::Grok,
        model: "test-model".to_owned(),
        commands,
        session_permits: Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY)),
        turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
        outer_permits: Arc::new(tokio::sync::Semaphore::new(1)),
        turn_capacity: TURN_QUEUE_CAPACITY,
        events: Arc::new(ThreadEventDispatcher::default()),
        alive: Arc::new(AtomicBool::new(true)),
        cooldown: Arc::new(AtomicBool::new(false)),
        driver: DriverThread::completed(),
    };
    tokio::spawn(async move {
        drop_cancel_turn_response(&mut receiver).await;
    });

    assert!(
        agent.cancel_turn("session").await.is_ok(),
        "dropped ACP cancel response must settle instead of leaving TaskStop failing"
    );
}

#[tokio::test]
async fn shutdown_is_idempotent_when_the_driver_is_already_unavailable() {
    let agent = GrokAcp::stopped_for_test();

    agent.shutdown().await;
    assert!(!agent.is_alive());
    agent.shutdown().await;
}

#[tokio::test]
async fn shutdown_waits_for_an_available_driver_to_acknowledge() {
    let (commands, receiver) = tokio::sync::mpsc::channel(1);
    let agent = GrokAcp {
        provider: AcpProvider::Grok,
        model: "test-model".to_owned(),
        commands,
        session_permits: Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY)),
        turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
        outer_permits: Arc::new(tokio::sync::Semaphore::new(1)),
        turn_capacity: TURN_QUEUE_CAPACITY,
        events: Arc::new(ThreadEventDispatcher::default()),
        alive: Arc::new(AtomicBool::new(true)),
        cooldown: Arc::new(AtomicBool::new(false)),
        driver: DriverThread::completed(),
    };
    let driver = tokio::spawn(acknowledge_shutdown(receiver));

    agent.shutdown().await;
    driver.await.expect("driver task");
    assert!(!agent.is_alive());
}

#[tokio::test]
async fn shutdown_joins_cleanup_after_alive_is_false_for_every_provider_and_cancel_failure() {
    assert_shutdown_cleanup(AcpProvider::Grok, "cancel-error").await;
    assert_shutdown_cleanup(AcpProvider::Grok, "cancel-ignored").await;
    assert_shutdown_cleanup(AcpProvider::Grok, "cancel-timeout").await;
    assert_shutdown_cleanup(AcpProvider::Configured, "cancel-error").await;
    assert_shutdown_cleanup(AcpProvider::Configured, "cancel-ignored").await;
    assert_shutdown_cleanup(AcpProvider::Configured, "cancel-timeout").await;
    assert_shutdown_cleanup(AcpProvider::Copilot, "cancel-error").await;
    assert_shutdown_cleanup(AcpProvider::Copilot, "cancel-ignored").await;
    assert_shutdown_cleanup(AcpProvider::Copilot, "cancel-timeout").await;
}

async fn acknowledge_shutdown(mut receiver: tokio::sync::mpsc::Receiver<DriverCommand>) {
    let Some(DriverCommand::Shutdown { response }) = receiver.recv().await else {
        panic!("shutdown command expected");
    };
    response
        .send(())
        .expect("driver receives shutdown acknowledgement");
}

async fn assert_shutdown_cleanup(provider: AcpProvider, failure: &str) {
    use std::{sync::mpsc as std_mpsc, time::Duration};

    let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
    let (cleanup_started, cleanup_observed) = tokio::sync::oneshot::channel();
    let (release_cleanup, cleanup_release) = std_mpsc::channel();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test driver runtime");
        runtime.block_on(async move {
            acknowledge_shutdown_cleanup(&mut receiver, cleanup_started, cleanup_release).await;
        });
    });
    let agent = Arc::new(GrokAcp {
        provider,
        model: "test-model".to_owned(),
        commands,
        session_permits: Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY)),
        turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
        outer_permits: Arc::new(tokio::sync::Semaphore::new(1)),
        turn_capacity: TURN_QUEUE_CAPACITY,
        events: Arc::new(ThreadEventDispatcher::default()),
        alive: Arc::new(AtomicBool::new(false)),
        cooldown: Arc::new(AtomicBool::new(false)),
        driver: DriverThread::new(handle),
    });
    let shutting_down = Arc::clone(&agent);
    let mut shutdown = tokio::spawn(async move { shutting_down.shutdown().await });
    cleanup_observed
        .await
        .unwrap_or_else(|_| panic!("{provider:?} {failure}: cleanup did not start"));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut shutdown)
            .await
            .is_err(),
        "{provider:?} {failure}: shutdown returned before cleanup completed"
    );
    release_cleanup
        .send(())
        .unwrap_or_else(|_| panic!("{provider:?} {failure}: release cleanup"));
    tokio::time::timeout(Duration::from_secs(1), &mut shutdown)
        .await
        .unwrap_or_else(|_| panic!("{provider:?} {failure}: shutdown did not finish"))
        .unwrap_or_else(|error| panic!("{provider:?} {failure}: {error}"));
    assert!(agent.driver.is_joined(), "{provider:?} {failure}");
    agent.shutdown().await;
}

async fn acknowledge_shutdown_cleanup(
    receiver: &mut tokio::sync::mpsc::Receiver<DriverCommand>,
    cleanup_started: tokio::sync::oneshot::Sender<()>,
    cleanup_release: std::sync::mpsc::Receiver<()>,
) {
    let Some(DriverCommand::Shutdown { response }) = receiver.recv().await else {
        panic!("shutdown command expected");
    };
    cleanup_started
        .send(())
        .expect("observe cleanup start before release");
    cleanup_release
        .recv()
        .expect("release simulated process wait");
    response.send(()).expect("acknowledge completed cleanup");
}

#[tokio::test]
async fn bounded_queues_apply_backpressure_at_fixed_capacities() {
    let (commands, mut command_receiver) = tokio::sync::mpsc::channel(COMMAND_QUEUE_CAPACITY);
    for _ in 0..COMMAND_QUEUE_CAPACITY {
        let (response, _) = tokio::sync::oneshot::channel();
        commands
            .send(DriverCommand::StartTurn {
                params: json!({}),
                permit: Arc::new(tokio::sync::Semaphore::new(1))
                    .acquire_owned()
                    .await
                    .unwrap(),
                response,
            })
            .await
            .unwrap();
    }
    assert_eq!(commands.capacity(), 0);
    assert_eq!(command_receiver.len(), COMMAND_QUEUE_CAPACITY);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), commands.reserve())
            .await
            .is_err()
    );
    command_receiver.recv().await.unwrap();
    assert!(commands.reserve().await.is_ok());

    let permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
    let (turns, mut turn_receiver) = tokio::sync::mpsc::channel(TURN_QUEUE_CAPACITY);
    for index in 0..TURN_QUEUE_CAPACITY {
        turns
            .send(PreparedTurn {
                session_id: format!("session-{index}"),
                model: "model".to_owned(),
                prompt: "prompt".to_owned(),
                effort: None,
                cancellation: pending_cancellation(),
                _permit: Arc::clone(&permits).acquire_owned().await.unwrap(),
            })
            .await
            .unwrap();
    }
    assert_eq!(permits.available_permits(), 0);
    assert_eq!(turns.capacity(), 0);
    assert_eq!(turn_receiver.len(), TURN_QUEUE_CAPACITY);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            Arc::clone(&permits).acquire_owned(),
        )
        .await
        .is_err()
    );
    turn_receiver.recv().await.unwrap();
    assert!(Arc::clone(&permits).acquire_owned().await.is_ok());
}

#[tokio::test]
async fn queue_turn_replaces_same_session_in_flight_turn() {
    let permits = Arc::new(tokio::sync::Semaphore::new(3));
    let instructions = std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
    let active = ActiveTurns::default();
    let invalidated = InvalidatedSessions::default();
    let (turns, mut receiver) = tokio::sync::mpsc::channel(2);
    let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
    active
        .borrow_mut()
        .insert("session".to_owned(), Some(cancel));

    let settle = async {
        let request = cancel_rx
            .await
            .expect("same-session follow-up must cancel the in-flight turn");
        assert!(request.response.send(Ok(())).is_ok());
        active.borrow_mut().remove("session");
    };
    let queued = queue_turn(
        AcpProvider::Configured,
        json!({"threadId":"session","input":"follow-up"}),
        Arc::clone(&permits).acquire_owned().await.unwrap(),
        &instructions,
        &turns,
        &active,
        &invalidated,
    );
    let (queued, ()) = tokio::join!(queued, settle);
    queued.expect("same-session follow-up must replace in-flight ACP turn");
    let turn = receiver.recv().await.expect("replacement turn");
    assert_eq!(turn.session_id, "session");
    assert_eq!(turn.prompt, "follow-up");
    assert!(active.borrow().contains_key("session"));
}

#[tokio::test]
async fn queue_turn_does_not_replace_a_different_session() {
    let permits = Arc::new(tokio::sync::Semaphore::new(3));
    let instructions = std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
    let active = ActiveTurns::default();
    let invalidated = InvalidatedSessions::default();
    let (turns, mut receiver) = tokio::sync::mpsc::channel(2);
    let (cancel, _cancel_rx) = tokio::sync::oneshot::channel();
    active
        .borrow_mut()
        .insert("session-a".to_owned(), Some(cancel));

    queue_turn(
        AcpProvider::Configured,
        json!({"threadId":"session-b","input":"independent"}),
        Arc::clone(&permits).acquire_owned().await.unwrap(),
        &instructions,
        &turns,
        &active,
        &invalidated,
    )
    .await
    .expect("independent session must queue without replacing a peer");

    assert!(active.borrow().contains_key("session-a"));
    assert!(active.borrow().contains_key("session-b"));
    let turn = receiver.recv().await.expect("independent turn");
    assert_eq!(turn.session_id, "session-b");
    assert_eq!(turn.prompt, "independent");
}

#[tokio::test]
async fn rejects_invalid_duplicate_and_unavailable_turn_queues() {
    let permits = Arc::new(tokio::sync::Semaphore::new(3));
    let instructions = std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
    let active = ActiveTurns::default();
    let invalidated = InvalidatedSessions::default();
    invalidated.borrow_mut().insert("invalid".to_owned());
    let (turns, receiver) = tokio::sync::mpsc::channel(1);
    let params = |id| json!({"threadId":id,"input":"prompt"});

    assert!(
        queue_turn(
            AcpProvider::Grok,
            params("invalid"),
            Arc::clone(&permits).acquire_owned().await.unwrap(),
            &instructions,
            &turns,
            &active,
            &invalidated,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("invalidated")
    );
    active.borrow_mut().insert("duplicate".to_owned(), None);
    assert!(
        queue_turn(
            AcpProvider::Copilot,
            params("duplicate"),
            Arc::clone(&permits).acquire_owned().await.unwrap(),
            &instructions,
            &turns,
            &active,
            &invalidated,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("active turn")
    );
    drop(receiver);
    assert!(
        queue_turn(
            AcpProvider::Grok,
            params("closed"),
            Arc::clone(&permits).acquire_owned().await.unwrap(),
            &instructions,
            &turns,
            &active,
            &invalidated,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("unavailable")
    );
}

#[tokio::test]
async fn handles_absent_repeated_and_dropped_turn_cancellations() {
    let active = ActiveTurns::default();
    let (response, result) = tokio::sync::oneshot::channel();
    cancel_turn(&active, "missing", response);
    assert!(result.await.unwrap().is_ok());

    let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
    active
        .borrow_mut()
        .insert("active".to_owned(), Some(cancel));
    let (first, first_result) = tokio::sync::oneshot::channel();
    cancel_turn(&active, "active", first);
    let request = cancel_rx.await.unwrap();
    request.response.send(Ok(())).unwrap();
    assert!(first_result.await.unwrap().is_ok());
    let (second, second_result) = tokio::sync::oneshot::channel();
    cancel_turn(&active, "active", second);
    assert!(second_result.await.unwrap().is_err());

    let (dropped, dropped_rx) = tokio::sync::oneshot::channel();
    drop(dropped_rx);
    active
        .borrow_mut()
        .insert("dropped".to_owned(), Some(dropped));
    let (response, result) = tokio::sync::oneshot::channel();
    cancel_turn(&active, "dropped", response);
    assert!(result.await.unwrap().is_ok());
}

#[tokio::test]
async fn cancels_a_queued_turn_when_its_requester_disconnects() {
    let active = ActiveTurns::default();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    active
        .borrow_mut()
        .insert("session".to_owned(), Some(cancel));
    let (response, requester) = tokio::sync::oneshot::channel();
    drop(requester);
    queue::finish_start_turn(&active, "session", response, Ok(()));
    assert!(cancelled.await.is_ok());

    let (response, requester) = tokio::sync::oneshot::channel();
    drop(requester);
    queue::finish_start_turn(
        &active,
        "missing",
        response,
        Err(anyhow::anyhow!("queue rejected")),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn start_turn_scheduler_keeps_cancellation_progress_independent() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let instructions =
                std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
            let active_turns = ActiveTurns::default();
            let invalidated_sessions = InvalidatedSessions::default();
            let (turns, mut turn_receiver) =
                tokio::sync::mpsc::channel::<PreparedTurn>(TURN_QUEUE_CAPACITY);
            let (start_turns, start_turn_receiver) =
                tokio::sync::mpsc::unbounded_channel::<StartTurnRequest>();
            let scheduler = tokio::task::spawn_local(drive_start_turns(
                AcpProvider::Grok,
                start_turn_receiver,
                std::rc::Rc::clone(&instructions),
                turns.clone(),
                std::rc::Rc::clone(&active_turns),
                std::rc::Rc::clone(&invalidated_sessions),
            ));

            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
            active_turns
                .borrow_mut()
                .insert("session".to_owned(), Some(cancel_tx));
            let permits = Arc::new(tokio::sync::Semaphore::new(1));
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            start_turns
                .send(StartTurnRequest {
                    params: json!({"threadId":"session","input":"next"}),
                    permit: Arc::clone(&permits).acquire_owned().await.unwrap(),
                    response: response_tx,
                })
                .unwrap();

            let cancel = tokio::time::timeout(std::time::Duration::from_secs(1), cancel_rx)
                .await
                .expect("scheduler did not reach the in-flight cancellation")
                .expect("scheduler dropped the cancellation request");
            cancel.response.send(Ok(())).unwrap();
            active_turns.borrow_mut().remove("session");

            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), response_rx)
                    .await
                    .expect("scheduler did not finish after cancellation")
                    .unwrap()
                    .is_ok()
            );
            let prepared =
                tokio::time::timeout(std::time::Duration::from_secs(1), turn_receiver.recv())
                    .await
                    .expect("scheduler did not queue the replacement turn")
                    .expect("scheduler dropped the replacement turn");
            assert_eq!(prepared.session_id, "session");
            drop(prepared);
            assert_eq!(permits.available_permits(), 1);

            drop(start_turns);
            drop(turns);
            scheduler.await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn start_turn_scheduler_rejects_requests_after_worker_shutdown() {
    let (turns, receiver) = tokio::sync::mpsc::unbounded_channel();
    drop(receiver);
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let (response, rejected) = tokio::sync::oneshot::channel();

    schedule_start_turn(
        &turns,
        StartTurnRequest {
            params: json!({"threadId":"rejected"}),
            permit: permits.acquire_owned().await.unwrap(),
            response,
        },
    );

    assert!(rejected.await.unwrap().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn turn_worker_runs_local_tasks_concurrently_and_cleans_them_up() {
    let local = tokio::task::LocalSet::new();
    local.run_until(check_concurrent_turn_worker()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn turn_worker_logs_a_panicking_task_and_still_shuts_down() {
    let local = tokio::task::LocalSet::new();
    local.run_until(check_panicking_turn_worker()).await;
}

async fn check_panicking_turn_worker() {
    let permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
    let (turns, receiver) = tokio::sync::mpsc::channel(1);
    let worker = tokio::task::spawn_local(drive_turn_tasks(receiver, panic_turn));
    turns
        .send(PreparedTurn {
            session_id: "panic".to_owned(),
            model: "model".to_owned(),
            prompt: String::new(),
            effort: None,
            cancellation: pending_cancellation(),
            _permit: permits.acquire_owned().await.unwrap(),
        })
        .await
        .unwrap();
    drop(turns);
    worker.await.unwrap();
}

async fn panic_turn(_turn: PreparedTurn) {
    panic!("fixture turn panic");
}

async fn check_concurrent_turn_worker() {
    let permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
    let (turns, receiver) = tokio::sync::mpsc::channel(TURN_QUEUE_CAPACITY);
    let active = std::rc::Rc::new(std::cell::Cell::new(0));
    let peak = std::rc::Rc::new(std::cell::Cell::new(0));
    let both_started = std::rc::Rc::new(tokio::sync::Notify::new());
    let release = std::rc::Rc::new(tokio::sync::Notify::new());
    let worker = tokio::task::spawn_local(drive_turn_tasks(receiver, {
        let active = std::rc::Rc::clone(&active);
        let peak = std::rc::Rc::clone(&peak);
        let both_started = std::rc::Rc::clone(&both_started);
        let release = std::rc::Rc::clone(&release);
        move |_turn| {
            hold_turn(
                std::rc::Rc::clone(&active),
                std::rc::Rc::clone(&peak),
                std::rc::Rc::clone(&both_started),
                std::rc::Rc::clone(&release),
            )
        }
    }));
    for session_id in ["one", "two"] {
        turns
            .send(PreparedTurn {
                session_id: session_id.to_owned(),
                model: "model".to_owned(),
                prompt: String::new(),
                effort: None,
                cancellation: pending_cancellation(),
                _permit: Arc::clone(&permits).acquire_owned().await.unwrap(),
            })
            .await
            .unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), both_started.notified())
        .await
        .expect("turn tasks did not overlap");
    assert_eq!(peak.get(), 2);
    drop(turns);
    tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("turn worker did not abort active tasks")
        .unwrap();
    assert_eq!(permits.available_permits(), TURN_QUEUE_CAPACITY);
}

async fn hold_turn(
    active: std::rc::Rc<std::cell::Cell<usize>>,
    peak: std::rc::Rc<std::cell::Cell<usize>>,
    both_started: std::rc::Rc<tokio::sync::Notify>,
    release: std::rc::Rc<tokio::sync::Notify>,
) {
    let count = active.get() + 1;
    active.set(count);
    peak.set(peak.get().max(count));
    if count == 2 {
        both_started.notify_one();
    }
    release.notified().await;
    active.set(active.get() - 1);
}

fn pending_cancellation() -> tokio::sync::oneshot::Receiver<super::CancelRequest> {
    let (_sender, receiver) = tokio::sync::oneshot::channel();
    receiver
}

#[tokio::test]
async fn public_spawn_entry_points_report_a_missing_program() {
    let previous = std::env::var_os("CLAUDEX_GROK_PROGRAM");
    // No other unit test reads this provider-specific override.
    unsafe { std::env::set_var("CLAUDEX_GROK_PROGRAM", "/definitely/missing/grok") };
    let spawned = GrokAcp::spawn("model").await;
    if let Some(value) = previous {
        unsafe { std::env::set_var("CLAUDEX_GROK_PROGRAM", value) };
    } else {
        unsafe { std::env::remove_var("CLAUDEX_GROK_PROGRAM") };
    }
    assert!(spawned.is_err());

    assert!(
        GrokAcp::spawn_with_program(
            "model",
            "/definitely/missing/grok",
            std::env::current_dir().unwrap()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn native_grok_rejects_non_api_reasoning_efforts_before_launch() {
    for effort in ["mid", "xhigh", "max"] {
        let error = match GrokAcp::spawn_with_program_and_effort(
            "grok-4.5",
            effort,
            "/definitely/missing/grok",
            std::env::current_dir().unwrap(),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("invalid Grok effort reached process launch"),
        };
        assert!(
            error.to_string().contains("low, medium, or high"),
            "unexpected {effort} error: {error}"
        );
    }
}

fn permission_request(options: Vec<acp::PermissionOption>) -> acp::RequestPermissionRequest {
    acp::RequestPermissionRequest::new(
        "session",
        acp::ToolCallUpdate::new("tool", acp::ToolCallUpdateFields::new()),
        options,
    )
}

#[tokio::test]
async fn ignores_non_agent_non_text_and_empty_notification_chunks() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    for update in [
        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("user"),
        ))),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Image(
            acp::ImageContent::new("data", "image/png"),
        ))),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(""),
        ))),
    ] {
        updates::dispatch_notification(
            &events,
            &updates::ThoughtUnits::default(),
            acp::SessionNotification::new("session", update),
        );
    }
    updates::dispatch_notification(
        &events,
        &updates::ThoughtUnits::default(),
        acp::SessionNotification::new(
            "session",
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new("visible"),
            ))),
        ),
    );
    assert_eq!(receiver.recv().await.unwrap()["params"]["delta"], "visible");
}
