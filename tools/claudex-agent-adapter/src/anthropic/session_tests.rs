use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Once},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use super::{
    candidate_length, codex_tool_name, dynamic_tool, is_better_length, owns_tool_result,
    reservation::reserve_matching_session, session_turn::contains_context_window_marker,
    thread_start_params, tool_configuration, transcript_owns_tool_results,
    validate_tool_result_ownership,
};
use crate::anthropic::{
    Bridge, MessagesRequest, SelectedSession, Session, content::ToolResult,
    subscription_request::subscription_request_prompt,
};
use crate::{
    agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute},
    app_server::AppServer,
};

fn request(system: Value, tools: Vec<Value>) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system,
        messages: vec![json!({"role":"user","content":"hello"})],
        tools,
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn session(signature: &str, transcript: Vec<Value>) -> Arc<Session> {
    session_for_model("main-model", signature, transcript)
}

fn session_for_model(model: &str, signature: &str, transcript: Vec<Value>) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    session_with_slot(model, signature, transcript, slots)
}

fn session_with_slot(
    model: &str,
    signature: &str,
    transcript: Vec<Value>,
    slots: Arc<Semaphore>,
) -> Arc<Session> {
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: model.to_owned(),
        signature: Arc::from(signature),
        transcript: Mutex::new(transcript),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(std::collections::HashSet::new()),
        internal_tools: HashMap::new(),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    })
}

async fn mock_app_server(script: &str) -> (tempfile::TempDir, Arc<AppServer>) {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("create source home");
    std::fs::write(source.join("auth.json"), "{}").expect("write source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(&program, script).expect("write mock app-server");
    let mut permissions = std::fs::metadata(&program)
        .expect("mock app-server metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("make mock app-server executable");
    let app =
        AppServer::spawn_with_program("main", program, &source, &root.path().join("isolated"))
            .await
            .expect("start mock app-server");
    (root, app)
}

async fn mock_trace(path: &std::path::Path, expected: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let trace = std::fs::read_to_string(path)
                .ok()
                .map(|trace| {
                    trace
                        .lines()
                        .map(|line| serde_json::from_str(line).expect("mock trace JSON"))
                        .collect::<Vec<Value>>()
                })
                .unwrap_or_default();
            if trace.len() >= expected {
                return trace;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("mock trace timeout")
}

fn enable_warning_logs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_test_writer()
            .try_init();
    });
}

#[test]
fn configures_external_and_internal_tools_without_duplicates() {
    let tools = vec![
        json!({"name":"Read","description":"read","input_schema":{"type":"object"}}),
        json!({"description":"missing name"}),
    ];
    let configured = tool_configuration(
        &request(json!("cc_is_subagent=true"), tools),
        Some("advisor-model"),
        Some("collaborator-model"),
    );
    assert_eq!(configured.0.len(), 3);
    assert_eq!(configured.1["cc_Read_0"], "Read");
    assert_eq!(configured.2["advisor"], "advisor-model");
    assert_eq!(configured.2["claude_collaborator"], "collaborator-model");

    let explicit = vec![json!({
        "name":"claude_collaborator", "input_schema":{"type":"object"}
    })];
    let configured = tool_configuration(
        &request(json!("cc_is_subagent=true"), explicit),
        None,
        Some("ignored"),
    );
    assert_eq!(configured.0.len(), 1);
    assert!(configured.2.is_empty());
}

#[test]
fn configures_a_bounded_batch_tool_for_parallel_agents() {
    let tools = vec![json!({
        "name":"Agent", "description":"delegate",
        "input_schema":{"type":"object","properties":{"prompt":{"type":"string"}}}
    })];
    let configured = tool_configuration(&request(Value::Null, tools), None, None);
    assert_eq!(configured.0.len(), 2);
    let batch = configured
        .0
        .iter()
        .find(|tool| {
            tool["description"]
                .as_str()
                .is_some_and(|text| text.contains("two or more"))
        })
        .expect("parallel Agent batch tool");
    assert_eq!(batch["inputSchema"]["properties"]["tasks"]["minItems"], 2);
    assert_eq!(batch["inputSchema"]["properties"]["tasks"]["maxItems"], 40);
    assert!(configured.1.values().any(|name| name.ends_with(":Agent")));
}

#[test]
fn main_and_worker_sessions_keep_full_claude_code_tool_sets() {
    let routing = r#"Claudex routing for this turn: {"providers":{},"selected_agents":["claudex-deepseek","claudex-ollama-glm-5-2"],"selected_workers":[{"agent":"claudex-deepseek","model":"deepseek-model"}]} mandatory policy"#;
    let tools = vec![
        json!({"name":"Read","input_schema":{"type":"object"}}),
        json!({"name":"Bash","input_schema":{"type":"object"}}),
        json!({"name":"Edit","input_schema":{"type":"object"}}),
        json!({"name":"Agent","input_schema":{"type":"object","properties":{"subagent_type":{"type":"string"},"prompt":{"type":"string"}}}}),
        json!({"name":"SendMessage","input_schema":{"type":"object"}}),
        json!({"name":"TaskGet","input_schema":{"type":"object"}}),
    ];
    let main = tool_configuration(
        &request(json!(routing), tools.clone()),
        Some("advisor"),
        None,
    );
    let exposed = main.1.values().cloned().collect::<Vec<_>>();
    assert!(exposed.iter().any(|name| name == "Agent"));
    assert!(exposed.iter().any(|name| name.ends_with(":Agent")));
    assert!(exposed.iter().any(|name| name == "SendMessage"));
    assert!(exposed.iter().any(|name| name == "TaskGet"));
    for tool_name in ["Read", "Bash", "Edit"] {
        assert!(exposed.iter().any(|name| name == tool_name));
    }
    let agent = main
        .0
        .iter()
        .find(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("cc_Agent_"))
        })
        .expect("routed Agent tool");
    assert_eq!(
        agent["inputSchema"]["properties"]["subagent_type"]["enum"],
        json!(["claudex-deepseek", "claudex-ollama-glm-5-2"])
    );
    assert!(
        agent["inputSchema"]["required"]
            .as_array()
            .expect("Agent required fields")
            .contains(&json!("claudex_model"))
    );

    let worker = tool_configuration(
        &request(json!(format!("cc_is_subagent=true\n{routing}")), tools),
        None,
        None,
    );
    assert!(worker.1.values().any(|name| name == "Read"));
    assert!(worker.1.values().any(|name| name == "Bash"));
    assert!(worker.1.values().any(|name| name == "Edit"));
    let nested_agent_schemas = worker
        .0
        .iter()
        .filter_map(|tool| {
            tool.pointer("/inputSchema/properties/subagent_type/enum")
                .or_else(|| {
                    tool.pointer(
                        "/inputSchema/properties/tasks/items/properties/subagent_type/enum",
                    )
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(nested_agent_schemas.len(), 2);
    for schema in nested_agent_schemas {
        assert_eq!(
            schema,
            &json!(["claudex-deepseek", "claudex-ollama-glm-5-2"])
        );
    }
}

#[test]
fn constrains_agent_schemas_to_the_latest_routing_context() {
    let tools = vec![json!({
        "name":"Agent",
        "input_schema":{"type":"object","properties":{"subagent_type":{"type":"string"}}}
    })];
    let mut request = request(Value::Null, tools);
    request.messages = vec![
        json!({"role":"user","content":r#"{"providers":{},"selected_agents":["claudex-old"]}"#}),
        json!({"role":"user","content":r#"{"providers":{},"selected_agents":["claudex-current"]}"#}),
    ];
    let configured = tool_configuration(&request, None, None);
    assert!(configured.0.iter().all(|tool| {
        tool.pointer("/inputSchema/properties/subagent_type/enum")
            .or_else(|| {
                tool.pointer("/inputSchema/properties/tasks/items/properties/subagent_type/enum")
            })
            == Some(&json!(["claudex-current"]))
    }));
}

#[test]
fn preserves_claude_code_agent_types_when_adding_routed_workers() {
    let standard_types = json!(["general-purpose", "Explore"]);
    let tools = ["Agent", "Task"]
        .into_iter()
        .map(|name| {
            json!({
                "name":name,
                "input_schema":{"type":"object","properties":{
                    "subagent_type":{"type":"string","enum":standard_types},
                    "prompt":{"type":"string"}
                }}
            })
        })
        .collect();
    let configured = tool_configuration(
        &request(
            json!(r#"{"providers":{},"selected_agents":["claudex-worker"]}"#),
            tools,
        ),
        None,
        None,
    );
    let expected = json!(["general-purpose", "Explore", "claudex-worker"]);
    for tool in &configured.0 {
        let schema = tool
            .pointer("/inputSchema/properties/subagent_type/enum")
            .or_else(|| {
                tool.pointer("/inputSchema/properties/tasks/items/properties/subagent_type/enum")
            })
            .expect("Agent or Task schema");
        assert_eq!(schema, &expected);
    }
}

#[test]
fn tolerates_a_routed_agent_schema_without_subagent_type() {
    let mut request = request(
        json!(r#"{"providers":{},"selected_agents":["worker"]}"#),
        vec![json!({
            "name":"Agent",
            "input_schema":{"type":"object","properties":{"prompt":{"type":"string"}}}
        })],
    );
    request.messages = vec![json!({"role":"user","content":"delegate"})];
    let configured = tool_configuration(&request, None, None);
    assert_eq!(configured.0.len(), 2);

    request.messages = vec![json!({
        "role":"user",
        "content":"Use model-x for this worker"
    })];
    let configured = tool_configuration(&request, None, None);
    assert_eq!(configured.0.len(), 2);

    request.system = json!(
        r#"{"providers":{"vendor":{"disabled":false,"agent":"worker","model":"model-x"}},"selected_agents":["worker"]}"#
    );
    request.tools = vec![json!({
        "name":"Agent",
        "input_schema":{"type":"object","properties":{"subagent_type":{"type":"string"}}}
    })];
    request.messages = vec![json!({
        "role":"user",
        "content":"Use model-x for this worker"
    })];
    let configured = tool_configuration(&request, None, None);
    let agent_schema = configured
        .0
        .iter()
        .find_map(|tool| tool.pointer("/inputSchema/properties/subagent_type/enum"))
        .expect("routed Agent schema");
    assert_eq!(agent_schema, &json!(["worker"]));
}

#[test]
fn adds_explicit_non_denied_provider_agents_to_the_routed_schema() {
    let routing = r#"Claudex routing for this turn: {"providers":{"vendor":{"available":false,"disabled":false,"agent":"claudex-vendor","model":"vendor-default","model_prefixes":[]},"codex":{"available":false,"disabled":false,"agent":"claudex-codex","model":"gpt-default","model_prefixes":["gpt-"]},"special":{"available":false,"disabled":false,"agent":"claudex-special","model":"vendor@beta+1","model_prefixes":[]},"summary":{"available":false,"disabled":false,"agent":"claudex-summary-only","model":"summary-only","model_prefixes":[]},"grok":{"available":false,"disabled":true,"agent":"claudex-grok","model":"grok-denied","model_prefixes":["grok-"]},"qwen":{"available":false,"disabled":false,"agent":"claudex-qwen","model":"qwen-denied","model_prefixes":["qwen-"]}},"selected_agents":["claudex-selected"],"disabled_subagent_models":["qwen-denied"]} mandatory policy"#;
    let tools = vec![json!({
        "name":"Agent",
        "input_schema":{"type":"object","properties":{
            "subagent_type":{"type":"string"},"prompt":{"type":"string"}
        }}
    })];
    let mut request = request(Value::Null, tools);
    request.messages = vec![json!({
        "role":"user",
        "content":format!("Use vendor-default, vendor@beta+1, and gpt-experimental. Do not bypass grok-denied or qwen-denied.\n{routing}")
    })];
    request
        .disabled_subagent_models
        .insert("qwen-denied".to_owned());

    let configured = tool_configuration(&request, None, None);
    let expected = json!([
        "claudex-selected",
        "claudex-codex",
        "claudex-special",
        "claudex-vendor"
    ]);
    let ordinary = configured
        .0
        .iter()
        .find_map(|tool| tool.pointer("/inputSchema/properties/subagent_type/enum"))
        .expect("ordinary routed agent enum");
    let batch = configured
        .0
        .iter()
        .find_map(|tool| {
            tool.pointer("/inputSchema/properties/tasks/items/properties/subagent_type/enum")
        })
        .expect("batch routed agent enum");

    assert_eq!(ordinary, &expected);
    assert_eq!(batch, &expected);
    assert!(
        ordinary
            .as_array()
            .expect("routed agent candidates")
            .iter()
            .all(|candidate| !matches!(
                candidate.as_str(),
                Some("claudex-grok" | "claudex-qwen" | "claudex-summary-only")
            ))
    );
}

#[test]
fn builds_thread_configuration_for_empty_and_team_system_prompts() {
    assert_empty_thread_configuration();
    assert_team_thread_configuration();
}

fn assert_empty_thread_configuration() {
    let empty = thread_start_params(&request(Value::Null, Vec::new()), "main", Vec::new());
    let base = empty["baseInstructions"]
        .as_str()
        .expect("base instructions");
    assert_eq!(base, empty["developerInstructions"]);
    assert_eq!(empty["sandbox"], "workspace-write");
    assert_eq!(empty["config"]["features"]["multi_agent"], false);
    assert_eq!(empty["config"]["features"]["shell_tool"], false);
    assert_eq!(empty["config"]["features"]["unified_exec"], false);
    let developer = empty["developerInstructions"]
        .as_str()
        .expect("developer instructions");
    assert_developer_guidance(developer);
}

fn assert_developer_guidance(developer: &str) {
    assert!(
        developer
            .contains("never infer from it that Claude Code or its SubAgent tasks are read-only")
    );
    assert!(developer.contains("do not copy restrictions from an unrelated earlier task"));
    assert!(
        developer.contains("preserve that authority in SubAgent prompts"),
        "implementation authority must propagate to SubAgents"
    );
    assert!(developer.contains("run independent calls, fetches, or checks in parallel"));
    assert!(developer.contains("Promise.all"));
    assert!(developer.contains("avoid serializing independent operations"));
    assert!(
        developer.contains("unless they are explicitly active for the current task"),
        "explicit current-task restrictions must remain supported"
    );
    assert!(developer.contains("Omit the SubAgent name field for ordinary SubAgents"));
    assert!(developer.contains("only when the active user explicitly supplies that teammate name"));
    assert!(developer.contains("every Agent or Task launch, including a nested launch"));
    assert!(developer.contains("exact claudex_model and claudex_effort"));
    assert!(developer.contains("never use generic claude or blindly inherit"));
}

fn assert_team_thread_configuration() {
    let agent = json!({
        "name":"Agent", "description":"spawn",
        "input_schema":{"type":"object","properties":{}}
    });
    let with_team = thread_start_params(
        &request(json!("custom system"), vec![agent]),
        "main",
        Vec::new(),
    );
    assert!(
        with_team["baseInstructions"]
            .as_str()
            .expect("team base instructions")
            .starts_with("custom system\n\n")
    );
    assert!(
        with_team["developerInstructions"]
            .as_str()
            .expect("team developer instructions")
            .contains("SubAgent")
    );
}

#[test]
fn bridge_instructions_support_every_configured_provider() {
    let configured = thread_start_params(&request(Value::Null, Vec::new()), "main", Vec::new());
    assert!(
        configured["developerInstructions"]
            .as_str()
            .expect("developer instructions")
            .contains("models selected by the current routing context are supported")
    );
}

#[test]
fn subscription_prompt_keeps_external_provider_models_out_of_the_native_field() {
    let prompt = subscription_request_prompt(&request(json!("system"), Vec::new()));
    assert!(prompt.contains("external provider model ID in the native model field"));
}

#[test]
fn subscription_prompt_requires_atomic_parallel_launches() {
    let prompt = subscription_request_prompt(&request(json!("system"), Vec::new()));
    assert!(prompt.contains("same assistant message and tool round"));
    assert!(prompt.contains("exactly that many launch calls"));
    assert!(prompt.contains("queued to a busy worker does not add parallel capacity"));
    assert!(prompt.contains("end the turn promptly with concise user-visible status"));
}

#[test]
fn starts_codex_threads_in_the_request_working_directory() {
    let root = tempfile::tempdir().expect("request cwd fixture");
    let active_cwd = root.path().join("active-child");
    std::fs::create_dir(&active_cwd).expect("create active child cwd");
    let active_cwd = active_cwd
        .canonicalize()
        .expect("canonical active child cwd");
    let system = json!(format!(
        "Project policy\n- Primary working directory: {}\nBridge policy",
        active_cwd.display()
    ));

    let params = thread_start_params(&request(system, Vec::new()), "main", Vec::new());

    assert_eq!(params["cwd"].as_str(), active_cwd.to_str());

    let launch_cwd = root.path().canonicalize().expect("canonical launch cwd");
    let mut launched = request(json!("no embedded cwd"), Vec::new());
    launched.working_directory = Some(launch_cwd.clone());
    let params = thread_start_params(&launched, "main", Vec::new());
    assert_eq!(params["cwd"].as_str(), launch_cwd.to_str());
    assert_eq!(
        crate::anthropic::subscription_request::subscription_request_cwd(&launched).as_deref(),
        Some(launch_cwd.as_path())
    );
}

#[test]
fn supplies_a_default_dynamic_tool_schema() {
    let tool = json!({"name":"lookup"});
    let dynamic = dynamic_tool(&tool, "lookup").expect("dynamic tool");
    assert_eq!(dynamic["inputSchema"]["type"], "object");
    assert!(dynamic_tool(&json!({"name": 7}), "invalid").is_none());
    assert_eq!(codex_tool_name("", 0), "cc__0");
}

#[tokio::test]
async fn candidate_requires_the_signature_and_matching_transcript() {
    let first = json!({"role":"user","content":"first"});
    let owner = session("signature", vec![first.clone()]);
    assert_eq!(
        candidate_length(&owner, &Arc::from("other"), std::slice::from_ref(&first)).await,
        None
    );
    assert_eq!(
        candidate_length(
            &owner,
            &Arc::from("signature"),
            std::slice::from_ref(&first)
        )
        .await,
        Some(1)
    );
    assert_eq!(
        candidate_length(
            &owner,
            &Arc::from("signature"),
            &[json!({"role":"user","content":"different"})]
        )
        .await,
        None
    );
}

#[tokio::test]
async fn reserves_only_idle_matching_sessions_for_parallel_requests() {
    let message = json!({"role":"user","content":"parallel"});
    let active = session("signature", Vec::new());
    let active_gate = Arc::clone(&active.gate).lock_owned().await;
    assert!(
        reserve_matching_session(
            vec![Arc::clone(&active)],
            &Arc::from("signature"),
            std::slice::from_ref(&message),
        )
        .await
        .is_none()
    );
    drop(active_gate);
    let selected = reserve_matching_session(
        vec![active],
        &Arc::from("signature"),
        std::slice::from_ref(&message),
    )
    .await
    .expect("idle matching session");
    assert_eq!(selected.existing_len, 0);

    let first = json!({"role":"user","content":"first"});
    let second = json!({"role":"assistant","content":"second"});
    let longer = session("signature", vec![first.clone()]);
    let shorter = session("signature", Vec::new());
    let selected = reserve_matching_session(
        vec![longer, shorter],
        &Arc::from("signature"),
        &[first, second],
    )
    .await
    .expect("best matching session");
    assert_eq!(selected.existing_len, 1);
}

#[tokio::test]
async fn finds_busy_matching_session_for_outer_preempt() {
    use super::reservation::find_busy_matching_session;
    let message = json!({"role":"user","content":"follow-up"});
    let idle = session("signature", Vec::new());
    let busy = session("signature", Vec::new());
    let _gate = Arc::clone(&busy.gate).lock_owned().await;
    let found = find_busy_matching_session(
        vec![idle, Arc::clone(&busy)],
        &Arc::from("signature"),
        std::slice::from_ref(&message),
        Some("model"),
        None,
    )
    .await
    .expect("busy match");
    assert!(Arc::ptr_eq(&found.0, &busy));
    assert_eq!(found.1, 0);

    let fallback = session("signature", vec![message.clone()]);
    let _fallback_gate = Arc::clone(&fallback.gate).lock_owned().await;
    let found = find_busy_matching_session(
        vec![fallback.clone()],
        &Arc::from("different-signature"),
        std::slice::from_ref(&message),
        Some("main-model"),
        None,
    )
    .await
    .expect("busy fallback match");
    assert!(Arc::ptr_eq(&found.0, &fallback));
}

#[tokio::test]
async fn take_gate_after_preempt_drops_orphaned_assistant_tail() {
    use super::reservation::take_gate_after_preempt;
    let user = json!({"role":"user","content":"hi"});
    let orphan = json!({"role":"assistant","content":"partial"});
    let follow = json!({"role":"user","content":"again"});
    let target = session("signature", vec![user.clone(), orphan]);
    // Gate is free so take_gate can lock immediately (cancel already settled).
    let selected = take_gate_after_preempt(&target, &[user.clone(), follow.clone()])
        .await
        .expect("aligned session");
    assert_eq!(selected.existing_len, 1);
    assert_eq!(target.transcript.lock().await.as_slice(), [user]);
}

#[test]
fn validates_orphan_results_against_assistant_tool_uses() {
    let messages = vec![json!({
        "role":"assistant",
        "content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{}}]
    })];
    let result = |id: &str| crate::anthropic::content::ToolResult {
        tool_use_id: id.to_owned(),
        content_items: Vec::new(),
        is_error: false,
    };
    assert!(transcript_owns_tool_results(&messages, &[result("tool-1")]));
    assert!(!transcript_owns_tool_results(
        &messages,
        &[result("unknown")]
    ));
    assert!(!transcript_owns_tool_results(&[], &[]));
}

#[test]
fn recognizes_pending_and_consumed_tool_results() {
    let pending = HashMap::from([("pending".to_owned(), Value::Null)]);
    let consumed = std::collections::HashSet::from(["consumed".to_owned()]);

    assert!(owns_tool_result(&pending, &consumed, "pending"));
    assert!(owns_tool_result(&pending, &consumed, "consumed"));
    assert!(!owns_tool_result(&pending, &consumed, "unknown"));
    let result = |id: &str| crate::anthropic::content::ToolResult {
        tool_use_id: id.to_owned(),
        content_items: Vec::new(),
        is_error: false,
    };
    assert!(validate_tool_result_ownership(&pending, &consumed, &[result("pending")]).is_ok());
    let error = validate_tool_result_ownership(&pending, &consumed, &[result("unknown")])
        .expect_err("unknown result must be rejected");
    assert!(
        error
            .to_string()
            .contains("already consumed by another request")
    );
    assert!(is_better_length(None, 1));
    assert!(is_better_length(Some(1), 2));
    assert!(!is_better_length(Some(2), 1));
}

#[test]
fn classifies_context_window_errors() {
    assert!(contains_context_window_marker("context window exceeded"));
    assert!(contains_context_window_marker("ContextWindowExceeded"));
    assert!(contains_context_window_marker(
        "ran out of room in this conversation"
    ));
    assert!(!contains_context_window_marker("validation failed"));
    assert!(super::session_turn::is_context_window_exceeded(
        &anyhow::anyhow!("context limit reached")
    ));
}

#[test]
fn pending_tool_results_are_submitted_before_context_preemption() {
    assert!(super::should_preempt_for_context_limit(
        110_000,
        Some(110_000),
        false
    ));
    assert!(!super::should_preempt_for_context_limit(
        111_801,
        Some(110_000),
        true
    ));
    assert!(!super::should_preempt_for_context_limit(
        109_999,
        Some(110_000),
        false
    ));
    assert!(!super::should_preempt_for_context_limit(
        111_801, None, false
    ));
}

#[tokio::test]
async fn session_capacity_evicts_idle_sessions_before_rejecting_busy_capacity() {
    let backend = AgentBackend::spawn_routes(&[]);
    let mut bridge = Bridge::new_with_backend(backend, "main".to_owned());
    bridge.session_slots = Arc::new(Semaphore::new(1));
    bridge.sessions.lock().await.push(session_with_slot(
        "main",
        "idle",
        Vec::new(),
        Arc::clone(&bridge.session_slots),
    ));

    let reclaimed = bridge
        .acquire_session_slot()
        .await
        .expect("idle session releases its slot");
    assert!(bridge.sessions.lock().await.is_empty());
    drop(reclaimed);

    bridge.session_slots = Arc::new(Semaphore::new(0));
    let error = bridge
        .acquire_session_slot()
        .await
        .expect_err("busy capacity is rejected");
    assert!(error.to_string().contains("session capacity"));
}

#[tokio::test]
async fn removes_sessions_for_a_failed_model_backend() {
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: "failed".to_owned(),
        backend: BackendKind::ConfiguredAcp,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: None,
        model_prefixes: Vec::new(),
        acp: Some(AcpLaunch {
            program: "/definitely/missing/claudex-acp".to_owned(),
            arguments: Vec::new(),
        }),
    }]);
    assert!(
        backend
            .request("thread/start", json!({"model":"failed"}))
            .await
            .is_err()
    );
    let bridge = Bridge::new_with_backend(backend, "failed".to_owned());
    let failed = session_for_model("failed", "failed", Vec::new());
    let healthy = session_for_model("healthy", "healthy", Vec::new());
    bridge
        .sessions
        .lock()
        .await
        .extend([Arc::clone(&failed), Arc::clone(&healthy)]);

    bridge.remove_failed_model_sessions("failed").await;

    let sessions = bridge.sessions.lock().await;
    assert_eq!(sessions.len(), 1);
    assert!(Arc::ptr_eq(&sessions[0], &healthy));
}

#[tokio::test]
async fn recovers_a_context_limited_turn_with_the_previous_signature() {
    enable_warning_logs();
    let (_root, app) = mock_app_server(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread start\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"replacement\"}}}'\nwhile read line; do :; done\n",
    )
    .await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let previous = session("shared-signature", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&previous));
    let gate = Arc::clone(&previous.gate).lock_owned().await;
    let request = request(Value::Null, Vec::new());
    let (selected, extras) = bridge
        .recover_turn_start(
            SelectedSession {
                session: Arc::clone(&previous),
                existing_len: 3,
                recovered: true,
                gate,
            },
            Vec::new(),
            Err(anyhow::anyhow!("context window exceeded")),
            super::session_turn::StartContextRetry {
                request: &request,
                effort: None,
                advisor_model: None,
                collaborator_model: None,
                has_tool_results: false,
            },
        )
        .await
        .expect("restart and start replacement session");

    assert_eq!(extras, request.messages);
    assert_eq!(selected.session.thread_id, "replacement");
    assert_eq!(selected.session.signature.as_ref(), "shared-signature");
    assert_eq!(selected.existing_len, 0);
    assert!(!selected.recovered);
    let sessions = bridge.sessions.lock().await;
    assert_eq!(sessions.len(), 1);
    assert!(!Arc::ptr_eq(&sessions[0], &previous));
}

#[tokio::test]
async fn removes_sessions_when_a_replacement_turn_start_fails() {
    let backend = AgentBackend::spawn_routes(&[]);
    let bridge = Bridge::new_with_backend(backend, "main".to_owned());
    let failed = session("replacement", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&failed));
    let gate = Arc::clone(&failed.gate).lock_owned().await;

    let result = bridge
        .finish_turn_start(
            SelectedSession {
                session: failed,
                existing_len: 0,
                recovered: false,
                gate,
            },
            Vec::new(),
            Err(anyhow::anyhow!("replacement turn start failed")),
        )
        .await;

    assert!(result.is_err());
    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn reports_when_context_recovery_cannot_create_a_replacement() {
    let (_root, app) = mock_app_server(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread exit\nexit 0\n",
    )
    .await;
    assert!(app.request("force/exit", json!({})).await.is_err());
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let previous = session("signature", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&previous));
    let gate = Arc::clone(&previous.gate).lock_owned().await;
    let request = request(Value::Null, Vec::new());

    let result = bridge
        .restart_after_start_context_error(
            SelectedSession {
                session: previous,
                existing_len: 0,
                recovered: false,
                gate,
            },
            super::session_turn::StartContextRetry {
                request: &request,
                effort: None,
                advisor_model: None,
                collaborator_model: None,
                has_tool_results: false,
            },
            &anyhow::anyhow!("context window exceeded"),
        )
        .await;

    assert!(result.is_err());
    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn removes_the_selected_session_when_turn_start_fails_immediately() {
    let (_root, app) = mock_app_server(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread exit\nexit 0\n",
    )
    .await;
    assert!(app.request("force/exit", json!({})).await.is_err());
    for _ in 0..10 {
        if !app.is_alive() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!app.is_alive(), "mock app-server should be stopped");

    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let session = session("signature", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&session));
    let gate = Arc::clone(&session.gate).lock_owned().await;
    let result = bridge
        .start_selected_turn(
            &request(Value::Null, Vec::new()),
            1,
            None,
            SelectedSession {
                session,
                existing_len: 0,
                recovered: false,
                gate,
            },
            Vec::new(),
            None,
            None,
            false,
        )
        .await;
    let error = match result {
        Ok(_) => panic!("stopped app-server should reject turn start"),
        Err(error) => error,
    };

    assert!(!error.to_string().is_empty());
    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn starts_outer_turns_with_full_input_priority_and_retry() {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let trace = root.path().join("turns.jsonl");
    let script = format!(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
        trace.display()
    );
    let (_server_root, app) = mock_app_server(&script).await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let request = request(Value::Null, Vec::new());
    let session = session("signature", Vec::new());
    let gate = Arc::clone(&session.gate).lock_owned().await;

    let turn = bridge
        .start_selected_turn(
            &request,
            7,
            None,
            SelectedSession {
                session,
                existing_len: 0,
                recovered: false,
                gate,
            },
            Vec::new(),
            None,
            None,
            true,
        )
        .await
        .expect("start outer turn");
    assert!(turn.retry.is_some());

    let trace = mock_trace(&trace, 1).await;
    assert_eq!(trace[0]["method"], "turn/start");
    assert_eq!(trace[0]["params"]["priority"], "user");
    assert_eq!(
        trace[0]["params"]["input"],
        json!([{ "type":"text", "text":"hello" }])
    );
}

#[tokio::test]
async fn starts_recovered_tool_results_as_a_full_transcript() {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let trace = root.path().join("recovered.jsonl");
    let script = format!(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
        trace.display()
    );
    let (_server_root, app) = mock_app_server(&script).await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let request = request(Value::Null, Vec::new());
    let session = session("signature", Vec::new());
    let gate = Arc::clone(&session.gate).lock_owned().await;

    bridge
        .start_selected_turn(
            &request,
            7,
            None,
            SelectedSession {
                session,
                existing_len: 0,
                recovered: true,
                gate,
            },
            vec![ToolResult {
                tool_use_id: "restored-tool".to_owned(),
                content_items: Vec::new(),
                is_error: false,
            }],
            None,
            None,
            true,
        )
        .await
        .expect("start recovered turn");

    let trace = mock_trace(&trace, 1).await;
    assert_eq!(trace[0]["method"], "turn/start");
    assert_eq!(
        trace[0]["params"]["input"],
        json!([{ "type":"text", "text":"hello" }])
    );
}

#[tokio::test]
async fn starts_incremental_turn_for_a_replayed_tool_result() {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let trace = root.path().join("turns.jsonl");
    let script = format!(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
        trace.display()
    );
    let (_server_root, app) = mock_app_server(&script).await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let mut request = request(json!("cc_is_subagent=true"), Vec::new());
    request
        .messages
        .push(json!({"role":"user","content":"follow-up"}));
    let session = session("signature", Vec::new());
    session
        .consumed_tool_ids
        .lock()
        .await
        .insert("tool-1".to_owned());
    let gate = Arc::clone(&session.gate).lock_owned().await;

    bridge
        .start_selected_turn(
            &request,
            8,
            Some("high".to_owned()),
            SelectedSession {
                session,
                existing_len: 1,
                recovered: false,
                gate,
            },
            vec![ToolResult {
                tool_use_id: "tool-1".to_owned(),
                content_items: Vec::new(),
                is_error: false,
            }],
            None,
            None,
            false,
        )
        .await
        .expect("start replay follow-up");

    let trace = mock_trace(&trace, 1).await;
    assert_eq!(trace[0]["method"], "turn/start");
    assert_eq!(trace[0]["params"]["effort"], "high");
    assert!(trace[0]["params"].get("priority").is_none());
    assert_eq!(
        trace[0]["params"]["input"],
        json!([{ "type":"text", "text":"follow-up" }])
    );
}

#[tokio::test]
async fn submits_fresh_tool_results_without_starting_another_turn() {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let trace = root.path().join("tool-results.jsonl");
    let script = format!(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
        trace.display()
    );
    let (_server_root, app) = mock_app_server(&script).await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let request = request(Value::Null, Vec::new());
    let session = session("signature", Vec::new());
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!(99));
    let gate = Arc::clone(&session.gate).lock_owned().await;

    bridge
        .start_selected_turn(
            &request,
            9,
            None,
            SelectedSession {
                session,
                existing_len: 1,
                recovered: false,
                gate,
            },
            vec![ToolResult {
                tool_use_id: "tool-1".to_owned(),
                content_items: vec![json!({"type":"text","text":"done"})],
                is_error: false,
            }],
            None,
            None,
            false,
        )
        .await
        .expect("submit tool result");

    let trace = mock_trace(&trace, 1).await;
    assert_eq!(trace[0]["id"], 99);
    assert!(trace[0].get("method").is_none());
    assert_eq!(trace[0]["result"]["success"], true);
}

#[tokio::test]
async fn retries_completed_turns_on_a_new_session() {
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let trace = root.path().join("retry.jsonl");
    let script = format!(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nread create\nprintf '%s\\n' \"$create\" >> '{}'\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"retry\"}}}}}}'\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
        trace.display(),
        trace.display()
    );
    let (_server_root, app) = mock_app_server(&script).await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(app), "main".to_owned());
    let previous = session("signature", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&previous));
    let retry = super::super::ContextRetry {
        request: request(Value::Null, Vec::new()),
        effort: Some("high".to_owned()),
        advisor_model: None,
        collaborator_model: None,
    };

    let turn = bridge
        .retry_after_context_window(retry, &previous, 10)
        .await
        .expect("retry turn");
    assert_eq!(turn.session.thread_id, "retry");
    assert!(turn.retry.is_none());
    assert_eq!(bridge.sessions.lock().await.len(), 1);

    let trace = mock_trace(&trace, 2).await;
    assert_eq!(trace[0]["method"], "thread/start");
    assert_eq!(trace[1]["method"], "turn/start");
    assert_eq!(trace[1]["params"]["effort"], "high");
}
