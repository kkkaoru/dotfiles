#![allow(clippy::excessive_nesting)]

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Once},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use super::{
    candidate_length, codex_tool_name, dynamic_tool, is_better_length,
    is_idempotent_task_lifecycle_error, owns_tool_result, reservation::reserve_matching_session,
    session_turn::contains_context_window_marker, thread_start_params,
    thread_start_params_for_mode, tool_configuration, tool_configuration_for_mode,
    transcript_owns_tool_results, validate_tool_result_ownership,
};

#[test]
fn treats_unknown_task_lifecycle_ids_as_idempotent_success() {
    assert!(is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"Error: No task found with ID: bfn35ry3f"
    })]));
    assert!(is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"error: no task found with id: already-consumed"
    })]));
    assert!(is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"Error: Task ae3ee29fc4eb8e09b is not running (status: completed)"
    })]));
    assert!(is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"Task ae3ee29fc4eb8e09b is not running (status: completed)"
    })]));
    assert!(!is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"Error: provider unavailable"
    })]));
    assert!(!is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"Error: Task ae3ee29fc4eb8e09b is not running (status: failed)"
    })]));
    assert!(!is_idempotent_task_lifecycle_error(&[json!({
        "type":"text",
        "text":"shell output: Error: No task found with ID: unrelated"
    })]));
}
use crate::agent_backend::WebSearchMode;
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
        disabled_subagent_models: Default::default(),
        signature: Arc::from(signature),
        transcript: Mutex::new(transcript),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(std::collections::HashSet::new()),
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

fn write_mock_program(root: &tempfile::TempDir, script: &str) -> std::path::PathBuf {
    let program = root.path().join("mock-app-server");
    std::fs::write(&program, script).expect("write mock app-server");
    let mut permissions = std::fs::metadata(&program)
        .expect("mock app-server metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("make mock app-server executable");
    program
}

fn restore_environment(name: &str, previous: Option<std::ffi::OsString>) {
    // SAFETY: tests restore each process-wide override before asserting results.
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var(name, previous);
        } else {
            std::env::remove_var(name);
        }
    }
}

async fn mock_trace(path: &std::path::Path, expected: usize) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let trace = read_mock_trace(path);
            if trace.len() >= expected {
                return trace;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("mock trace timeout")
}

fn read_mock_trace(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .ok()
        .map(|trace| {
            trace
                .lines()
                .map(|line| serde_json::from_str(line).expect("mock trace JSON"))
                .collect()
        })
        .unwrap_or_default()
}

async fn wait_for_app_stop(app: &AppServer) {
    for _ in 0..10 {
        if !app.is_alive() {
            return;
        }
        tokio::task::yield_now().await;
    }
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
fn exposes_only_received_public_tools_without_internal_inference() {
    let tools = vec![
        json!({"name":"Read","description":"read","input_schema":{"type":"object"}}),
        json!({"description":"missing name"}),
    ];
    let configured = tool_configuration(
        &request(json!("cc_is_subagent=true"), tools),
        Some("advisor-model"),
        Some("collaborator-model"),
    );
    assert_eq!(configured.0.len(), 1);
    assert_eq!(configured.1["cc_Read_0"], "Read");
    assert!(configured.2.is_empty());

    let explicit = vec![json!({
        "name":"claude_collaborator", "input_schema":{"type":"object"}
    })];
    let configured = tool_configuration(
        &request(json!("cc_is_subagent=true"), explicit),
        None,
        Some("ignored"),
    );
    assert_eq!(configured.0.len(), 1);
    assert!(
        configured
            .1
            .values()
            .any(|name| name == "claude_collaborator")
    );
    assert!(configured.2.is_empty());
}

#[test]
fn does_not_synthesize_an_unrequested_batch_tool() {
    let tools = vec![json!({
        "name":"Agent", "description":"delegate",
        "input_schema":{"type":"object","properties":{"prompt":{"type":"string"}}}
    })];
    let configured = tool_configuration(&request(Value::Null, tools), None, None);
    assert_eq!(configured.0.len(), 1);
    assert_eq!(configured.1.len(), 1);
    assert!(configured.1.values().any(|name| name == "Agent"));
    assert!(!configured.1.values().any(|name| name.ends_with(":Agent")));
}

#[test]
fn agent_and_task_input_schemas_are_exactly_the_received_schemas() {
    let agent_schema = json!({
        "type":"object",
        "properties":{
            "prompt":{"type":"string","minLength":3,"description":"native prompt"},
            "subagent_type":{"type":"string","enum":["general-purpose","Explore"]},
            "run_in_background":{"type":"boolean","const":true}
        },
        "required":["prompt","subagent_type"],
        "additionalProperties":false,
        "x-native-contract":{"version":220}
    });
    let task_schema = json!({
        "oneOf":[
            {"type":"object","required":["description"]},
            {"type":"object","required":["prompt"]}
        ]
    });
    let request = request(
        json!(r#"{"providers":{},"selected_agents":["routed-worker"]}"#),
        vec![
            json!({"name":"Agent","description":"native Agent","input_schema":agent_schema}),
            json!({"name":"Task","description":"native Task","input_schema":task_schema}),
        ],
    );
    let (tools, names, internal) = tool_configuration(
        &request,
        Some("ignored-advisor"),
        Some("ignored-collaborator"),
    );

    assert_eq!(tools.len(), 2);
    assert!(internal.is_empty());
    assert!(
        !names
            .values()
            .any(|name| name.contains(":Agent") || name.contains(":Task"))
    );
    let forwarded = |original: &str| {
        let dynamic = names
            .iter()
            .find_map(|(dynamic, name)| (name == original).then_some(dynamic))
            .expect("received tool mapping");
        &tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(dynamic.as_str()))
            .expect("forwarded tool")["inputSchema"]
    };
    assert_eq!(forwarded("Agent"), &agent_schema);
    assert_eq!(forwarded("Task"), &task_schema);
}

#[test]
fn received_advisor_schema_is_public_instead_of_internal_execution() {
    let schema = json!({
        "type":"object",
        "properties":{"question":{"type":"string"}},
        "required":["question"],
        "additionalProperties":false
    });
    let request = request(
        Value::Null,
        vec![json!({"name":"advisor","input_schema":schema})],
    );
    let (tools, names, internal) = tool_configuration(&request, Some("hidden-model"), None);
    assert_eq!(tools.len(), 1);
    assert!(names.values().any(|name| name == "advisor"));
    assert!(internal.is_empty());
    assert_eq!(tools[0]["inputSchema"], schema);
}

#[test]
fn documents_idempotent_task_stop_semantics_in_the_dynamic_schema() {
    for name in ["TaskStop", "StopTask", "Stop Task"] {
        let tool = dynamic_tool(
            &json!({"name":name,"description":"stop a background task"}),
            "cc_task_stop_0",
        )
        .expect("task-stop schema");
        let description = tool["description"].as_str().expect("description");
        assert!(description.contains("stopping is idempotent"));
        assert!(description.contains("exact active task_id"));
        assert!(description.contains("No task found"));
    }
    let ordinary =
        dynamic_tool(&json!({"name":"TaskGet"}), "cc_task_get_0").expect("ordinary task schema");
    assert!(
        !ordinary["description"]
            .as_str()
            .expect("ordinary description")
            .contains("stopping is idempotent")
    );
}

#[test]
fn main_and_worker_sessions_preserve_received_tool_schemas_exactly() {
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
    assert!(!exposed.iter().any(|name| name.ends_with(":Agent")));
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
        agent["inputSchema"],
        json!({"type":"object","properties":{"subagent_type":{"type":"string"},"prompt":{"type":"string"}}})
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
                .or_else(|| tool.pointer("/inputSchema/properties/subagent_type"))
        })
        .collect::<Vec<_>>();
    assert_eq!(nested_agent_schemas, vec![&json!({"type":"string"})]);
}

#[test]
fn routing_context_never_mutates_received_agent_schema() {
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
    assert_eq!(configured.0.len(), 1);
    assert_eq!(
        configured.0[0]["inputSchema"],
        json!({"type":"object","properties":{"subagent_type":{"type":"string"}}})
    );
}

#[test]
fn preserves_claude_code_agent_types_without_adding_routed_workers() {
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
    let expected = json!(["general-purpose", "Explore"]);
    assert_eq!(configured.0.len(), 2);
    for tool in &configured.0 {
        let schema = tool
            .pointer("/inputSchema/properties/subagent_type/enum")
            .expect("Agent or Task schema");
        assert_eq!(schema, &expected);
    }
}

#[test]
fn model_names_in_prompt_do_not_expand_agent_definitions() {
    let tools = vec![json!({
        "name":"Agent",
        "input_schema":{"type":"object","properties":{
            "subagent_type":{"type":"string","enum":["general-purpose"]},
            "prompt":{"type":"string"}
        }}
    })];
    let request = request(json!(r#"{"providers":{},"selected_agents":[]}"#), tools);
    let mut request = request;
    request.messages = vec![json!({
        "role":"user",
        "content":"Use subagent_type=claudex-haiku-search with claudex_model=claude-haiku-4-5."
    })];
    let configured = tool_configuration(&request, None, None);
    let schema = configured
        .0
        .iter()
        .find_map(|tool| tool.pointer("/inputSchema/properties/subagent_type/enum"))
        .expect("Agent schema");
    assert_eq!(schema, &json!(["general-purpose"]));
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
    assert_eq!(configured.0.len(), 1);

    request.messages = vec![json!({
        "role":"user",
        "content":"Use model-x for this worker"
    })];
    let configured = tool_configuration(&request, None, None);
    assert_eq!(configured.0.len(), 1);

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
    assert_eq!(
        configured.0[0]["inputSchema"],
        json!({"type":"object","properties":{"subagent_type":{"type":"string"}}})
    );
}

#[test]
fn explicit_provider_models_do_not_mutate_agent_schema() {
    let routing = r#"Claudex routing for this turn: {"providers":{"vendor":{"available":false,"disabled":false,"agent":"claudex-vendor","model":"vendor-default","model_prefixes":[]},"codex":{"available":false,"disabled":false,"agent":"claudex-codex","model":"gpt-default","model_prefixes":["gpt-"]},"special":{"available":false,"disabled":false,"agent":"claudex-special","model":"vendor@beta+1","model_prefixes":[]},"summary":{"available":false,"disabled":false,"agent":"claudex-summary-only","model":"summary-only","model_prefixes":[]},"grok":{"available":false,"disabled":true,"agent":"claudex-grok","model":"grok-denied","model_prefixes":["grok-"]},"qwen":{"available":false,"disabled":false,"agent":"claudex-qwen","model":"qwen-denied","model_prefixes":["qwen-"]}},"selected_agents":["claudex-selected","claudex-qwen"],"selected_workers":[{"agent":"claudex-qwen","model":"qwen-denied"}],"disabled_subagent_models":["qwen-denied"]} mandatory policy"#;
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
    assert_eq!(configured.0.len(), 1);
    assert_eq!(configured.1.len(), 1);
    assert!(configured.1.values().any(|name| name == "Agent"));
    assert_eq!(
        configured.0[0]["inputSchema"],
        json!({"type":"object","properties":{
            "subagent_type":{"type":"string"},"prompt":{"type":"string"}
        }})
    );
}

#[test]
fn builds_thread_configuration_for_empty_and_team_system_prompts() {
    assert_empty_thread_configuration();
    assert_team_thread_configuration();
}

#[test]
fn enables_native_search_without_exposing_a_duplicate_dynamic_tool() {
    let search = json!({
        "name": "WebSearch",
        "description": "search",
        "input_schema": {"type":"object"}
    });
    let (tools, names, _) = tool_configuration_for_mode(
        &request(Value::Null, vec![search]),
        None,
        None,
        WebSearchMode::CodexNative,
    );
    assert!(tools.iter().all(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.contains("WebSearch"))
    }));
    assert!(!names.values().any(|name| *name == "WebSearch"));
    let params = thread_start_params_for_mode(
        &request(
            Value::Null,
            vec![json!({"name":"WebSearch","input_schema":{"type":"object"}})],
        ),
        "gpt-native",
        Vec::new(),
        WebSearchMode::CodexNative,
    );
    assert_eq!(params["config"]["web_search"], "live");
    assert_eq!(params["config"]["features"]["web_search"], true);
}

#[test]
fn main_session_does_not_synthesize_agent_tools() {
    let request = request(
        json!("main session"),
        vec![json!({
            "name":"WebFetch",
            "input_schema":{"type":"object"}
        })],
    );
    let (_, names, _) =
        tool_configuration_for_mode(&request, None, None, WebSearchMode::CodexNative);
    assert_eq!(
        names.values().collect::<Vec<_>>(),
        vec![&"WebFetch".to_owned()]
    );
}

#[test]
fn resumed_codex_request_does_not_infer_tools_from_history() {
    let mut request = request(json!("resumed main session"), Vec::new());
    request.messages = vec![json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "name": "Bash", "input": {}}]
    })];
    let (_, names, _) = tool_configuration(&request, None, None);
    assert!(names.is_empty());
}

#[test]
fn failed_resume_without_tool_history_stays_toolless() {
    let request = request(json!("resumed main session"), Vec::new());
    let (dynamic_tools, external_names, _) = tool_configuration(&request, None, None);
    assert!(external_names.is_empty());
    assert!(dynamic_tools.is_empty());
}

#[test]
fn received_agent_tool_is_forwarded_exactly_without_adding_task_or_routing_fields() {
    let request = request(
        json!("main session"),
        vec![json!({
            "name":"Agent",
            "description":"native agent",
            "input_schema":{
                "type":"object",
                "properties":{"prompt":{"type":"string"}},
                "required":["prompt"],
                "additionalProperties":false
            }
        })],
    );
    let (tools, names, _) = tool_configuration(&request, None, None);
    assert!(!names.values().any(|name| name == "Task"));
    let dynamic_name = names
        .iter()
        .find_map(|(dynamic, name)| (name == "Agent").then_some(dynamic))
        .expect("received Agent tool");
    let schema = tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some(dynamic_name.as_str()))
        .expect("dynamic Agent schema");
    assert_eq!(
        schema["inputSchema"],
        json!({
            "type":"object",
            "properties":{"prompt":{"type":"string"}},
            "required":["prompt"],
            "additionalProperties":false
        })
    );
    assert!(
        schema["inputSchema"]
            .pointer("/properties/claudex_model")
            .is_none()
    );
    assert!(
        schema["inputSchema"]
            .pointer("/properties/claudex_effort")
            .is_none()
    );
    assert_eq!(tools.len(), 1);
}

#[test]
fn hides_only_new_native_launch_tools_after_the_session_budget_is_reached() {
    let mut request = request(
        json!("main session"),
        vec![
            json!({"name":"Read","input_schema":{"type":"object"}}),
            json!({"name":"Agent","input_schema":{"type":"object"}}),
            json!({"name":"Task","input_schema":{"type":"object"}}),
            json!({"name":"SendMessage","input_schema":{"type":"object"}}),
        ],
    );
    request.metadata = json!({"_claudex_subagent_spawn_limit_reached":true});
    let (tools, names, _) = tool_configuration(&request, None, None);
    assert!(
        !names
            .values()
            .any(|name| matches!(name.as_str(), "Agent" | "Task"))
    );
    assert!(names.values().any(|name| name == "Read"));
    assert!(names.values().any(|name| name == "SendMessage"));
    assert_eq!(tools.len(), 2);
}

#[test]
fn search_worker_preserves_every_received_capability() {
    let mut request = request(
        json!("cc_is_subagent=true; Dedicated live-web retrieval worker: claudex-haiku-search"),
        vec![
            json!({"name":"Read","input_schema":{"type":"object"}}),
            json!({"name":"Agent","input_schema":{"type":"object"}}),
            json!({"name":"WebSearch","input_schema":{"type":"object"}}),
            json!({"name":"WebFetch","input_schema":{"type":"object"}}),
        ],
    );
    request.messages = vec![json!({
        "role":"user",
        "content":"Use the claudex-haiku-search live-web retrieval worker."
    })];
    let (tools, names, _) =
        tool_configuration_for_mode(&request, None, None, WebSearchMode::CodexNative);
    assert_eq!(tools.len(), 3, "Read, Agent, and WebFetch remain");
    for expected in ["Read", "Agent", "WebFetch"] {
        assert!(names.values().any(|name| name == expected));
    }
    assert!(!names.values().any(|name| name.ends_with(":Agent")));
}

fn assert_empty_thread_configuration() {
    let empty = thread_start_params(&request(Value::Null, Vec::new()), "main", Vec::new());
    let base = empty["baseInstructions"]
        .as_str()
        .expect("base instructions");
    assert_eq!(base, empty["developerInstructions"]);
    assert_eq!(empty["sandbox"], "danger-full-access");
    assert_eq!(empty["config"]["features"]["multi_agent"], false);
    assert_eq!(empty["config"]["features"]["shell_tool"], true);
    assert_eq!(empty["config"]["features"]["tool_search"], true);
    assert_eq!(empty["config"]["features"]["unified_exec"], true);
    let developer = empty["developerInstructions"]
        .as_str()
        .expect("developer instructions");
    assert_developer_guidance(developer);
}

fn assert_developer_guidance(developer: &str) {
    const REQUIRED: &[&str] = &[
        "never infer from it that Claude Code or its SubAgent tasks are read-only",
        "do not copy restrictions from an unrelated earlier task",
        "preserve that authority in SubAgent prompts",
        "explicitly requires live WebSearch",
        "run independent calls, fetches, or checks in parallel",
        "Promise.all",
        "avoid serializing independent operations",
        "unless they are explicitly active for the current task",
        "Omit the SubAgent name field for ordinary SubAgents",
        "only when the active user explicitly supplies that teammate name",
        "Use only fields present in the exact Agent or Task schema supplied by Claude Code",
        "never invent adapter-only claudex_model or claudex_effort",
        "never use generic claude or blindly inherit",
        "main session must control parallel distribution across multiple SubAgents",
        "Avoid serial heavy processing by one worker",
        "reuse compatible workers with SendMessage and the exact prior Agent/Task recipient instead of churning processes",
        "custom-advisor is a separate logical session singleton/capacity channel",
        "built-in advisor remains independent of worker capacity",
        "complex or ambiguous decisions",
        "worker stalls/timeouts",
        "consult one custom-advisor when triggered",
        "Prefer reusing a compatible recipient via SendMessage over launching a replacement process",
        "set run_in_background=true on every launch in the single batch",
        "Do not mix foreground and background launches in one batch",
        "end the current turn promptly instead of reasoning while waiting",
        "never wait for every background task before accepting another user instruction",
        "never call TaskOutput or TaskGet merely to drain pending notifications",
    ];
    for phrase in REQUIRED {
        assert!(
            developer.contains(phrase),
            "missing developer guidance: {phrase}"
        );
    }
}

#[test]
fn main_session_orchestration_instructions_are_omitted_for_subagents() {
    let mut subagent = request(
        json!("x-anthropic-billing-header: cc_version=1; cc_is_subagent=true;"),
        Vec::new(),
    );
    subagent.messages = vec![json!({
        "role":"user",
        "content":"<claudex-agent-id>toolu_subagent</claudex-agent-id>\ncontinue"
    })];
    let params = thread_start_params(&subagent, "worker", Vec::new());
    let developer = params["developerInstructions"]
        .as_str()
        .expect("developer instructions");
    assert!(
        !developer.contains("Claudex main-session orchestration mode is active"),
        "worker turns must not receive main-session orchestration mode"
    );
    assert!(developer.contains(
        "Prefer reusing a compatible recipient via SendMessage over launching a replacement process"
    ));
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
#[allow(clippy::cognitive_complexity)]
fn subscription_prompt_requires_atomic_parallel_launches() {
    let prompt = subscription_request_prompt(&request(json!("system"), Vec::new()));
    let minimum = crate::anthropic::agent_batch::minimum_batch_size();
    assert!(prompt.contains("same assistant message and tool round"));
    assert!(prompt.contains("exactly that many launch calls"));
    assert!(prompt.contains(&format!("at least {minimum}")));
    assert!(prompt.contains("ordinary workers"));
    assert!(prompt.contains("run_in_background=true"));
    assert!(prompt.contains("Do not mix foreground and background launches"));
    assert!(prompt.contains("queued to a busy worker does not add parallel capacity"));
    assert!(prompt.contains("end the turn promptly with concise user-visible status"));
    assert!(prompt.contains("never wait for every background task before accepting another user instruction"));
    assert!(prompt.contains("never call TaskOutput or TaskGet merely to drain pending notifications"));
    assert!(
        prompt
            .contains("main session must control parallel distribution across multiple SubAgents")
    );
    assert!(prompt.contains("Avoid serial heavy processing by one worker"));
    assert!(prompt.contains("Shared-workspace safety is mandatory"));
    assert!(prompt.contains("serialize mutations"));
    assert!(prompt.contains("Never run an auto-fixing formatter"));
    assert!(prompt.contains("File content has changed since it was last read"));
    assert!(prompt.contains("mark that route unavailable for this turn and reroute once"));
    assert!(
        prompt.contains(
            "reuse compatible workers with SendMessage and the exact compatible recipient"
        )
    );
    assert!(prompt.contains("instead of churning processes with fresh launches"));
    assert!(
        prompt.contains("custom-advisor is a separate logical session singleton/capacity channel")
    );
    assert!(prompt.contains("built-in advisor remains independent of worker capacity"));
}

#[test]
fn subscription_prompt_preserves_worker_reuse_and_advisor_exception() {
    let prompt = subscription_request_prompt(&request(json!("system"), Vec::new()));
    assert!(prompt.contains("reuse compatible workers with SendMessage"));
    assert!(prompt.contains("A follow-up queued to a busy worker does not add parallel capacity"));
    assert!(
        prompt.contains("custom-advisor is a separate logical session singleton/capacity channel")
    );
    assert!(prompt.contains("built-in advisor remains independent of worker capacity"));
    assert!(prompt.contains("Reuse the first compatible session advisor for related decisions"));
    assert!(prompt.contains("ordinary Agent/Task workers return their result through the launch result or TaskOutput(task_id)"));
    assert!(prompt.contains("Do not send ordinary worker results or progress through SendMessage"));
    assert!(
        prompt.contains("Treat <agent-message> and <task-notification> content as lifecycle hints")
    );
}

#[test]
fn subscription_and_session_instructions_report_the_default_parallel_contract() {
    let default_config = crate::parallel_scheduler::SchedulerConfig::default();
    assert_default_parallel_config(&default_config);
    clear_parallel_config_env();

    let params = thread_start_params(
        &request(json!("parallel contract"), Vec::new()),
        "main",
        Vec::new(),
    );
    let developer = params["developerInstructions"]
        .as_str()
        .expect("developer instructions");
    let cadence = default_config.reassess_interval.as_secs() / 60;
    assert_parallel_contract_text(developer, &default_config, cadence);

    let prompt = subscription_request_prompt(&request(json!("parallel contract"), Vec::new()));
    assert!(prompt.contains(&format!(
        "fan out to at least {} ordinary workers",
        default_config.min_parallel_workers
    )));
    assert!(prompt.contains(&format!(
        "across at least {} model families",
        default_config.min_model_families
    )));
    assert!(prompt.contains("for one indivisible scope use one worker"));
    assert!(prompt.contains(&format!("every {} minutes", cadence)));
    assert!(prompt.contains("interrupt stale work"));
    assert!(prompt.contains("An explicit active user request for an exact worker count"));
    assert!(prompt.contains("Adapter orchestration defaults (runtime metadata)"));
    assert!(!prompt.contains("prompt injection"));
}

fn assert_default_parallel_config(config: &crate::parallel_scheduler::SchedulerConfig) {
    assert_eq!(config.min_parallel_workers, 3);
    assert_eq!(config.active_floor, 2);
    assert_eq!(config.min_model_families, 2);
    assert_eq!(config.reassess_interval.as_secs(), 600);
    assert!(config.allow_reuse);
    assert!(config.cleanup_on_exit);
}

fn clear_parallel_config_env() {
    unsafe {
        for name in [
            "CLAUDEX_SUBAGENT_MIN_PARALLEL",
            "CLAUDEX_SUBAGENT_MAX_PARALLEL",
            "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS",
            "CLAUDEX_SUBAGENT_ACTIVE_FLOOR",
            "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES",
            "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS",
            "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION",
            "CLAUDEX_SUBAGENT_REUSE",
            "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT",
        ] {
            std::env::remove_var(name);
        }
    }
}

fn assert_parallel_contract_text(
    text: &str,
    config: &crate::parallel_scheduler::SchedulerConfig,
    cadence: u64,
) {
    assert!(
        text.contains(
            "Runtime parallel policy: choose one ordinary worker for one indivisible scope"
        )
    );
    assert!(text.contains(&format!(
        "fan out to at least {} ordinary workers",
        config.min_parallel_workers
    )));
    assert!(text.contains(&format!(
        "fan out to at least {} ordinary workers",
        config.min_parallel_workers
    )));
    assert!(text.contains(&format!(
        "across at least {} model families",
        config.min_model_families
    )));
    assert!(text.contains(&format!("every {cadence} minutes")));
    assert!(text.contains("interrupt stale work"));
    assert_eq!(text.matches("Dynamic parallel status:").count(), 1);
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
async fn toolless_main_continuation_reuses_the_session_with_bash_schema() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut initial = request(
        json!("main system"),
        vec![json!({
            "name":"Bash",
            "description":"run shell commands",
            "input_schema":{"type":"object"}
        })],
    );
    initial.metadata = json!({"user_id":"continued-main"});
    initial.messages = vec![
        json!({"role":"user","content":"inspect the repository"}),
        json!({"role":"assistant","content":[{"type":"text","text":"I will inspect it."}]}),
    ];
    let (_, external_tools, _) = tool_configuration(&initial, None, None);
    let bash_dynamic_name = external_tools
        .iter()
        .find_map(|(dynamic, original)| (original == "Bash").then_some(dynamic.clone()))
        .expect("initial main session exposes Bash");
    let initial_signature = bridge.intern_signature(format!(
        "{}\0{}",
        bridge.request_model(&initial),
        crate::anthropic::content::request_signature(&initial, None, None)
            .expect("initial request signature")
    ));
    let mut retained = session_for_model("main", &initial_signature, initial.messages.clone());
    let retained_session = Arc::get_mut(&mut retained).expect("session is not yet shared");
    retained_session.external_tool_names = external_tools;
    retained_session.client_user_id = Some("continued-main".to_owned());
    bridge.sessions.lock().await.push(Arc::clone(&retained));

    let mut continued = initial.clone();
    continued.tools.clear();
    continued
        .messages
        .push(json!({"role":"user","content":"now check git status"}));
    let continued_signature = bridge.intern_signature(format!(
        "{}\0{}",
        bridge.request_model(&continued),
        crate::anthropic::content::request_signature(&continued, None, None)
            .expect("tool-less continuation signature")
    ));

    let selected = bridge
        .select_session(&continued, continued_signature, None, None, &[])
        .await
        .expect("tool-less continuation reuses the main thread");
    assert!(Arc::ptr_eq(&selected.session, &retained));
    assert_eq!(selected.existing_len, initial.messages.len());
    assert_eq!(
        selected.session.external_tool_names.get(&bash_dynamic_name),
        Some(&"Bash".to_owned())
    );
}

#[tokio::test]
async fn toolless_subagent_continuation_reuses_the_session_with_bash_schema() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut initial = request(
        json!("cc_is_subagent=true\n<claudex-agent-id>toolu_command_probe</claudex-agent-id>"),
        vec![json!({
            "name":"Bash",
            "description":"run shell commands",
            "input_schema":{"type":"object"}
        })],
    );
    initial.metadata = json!({"user_id":"continued-subagent"});
    initial.messages = vec![
        json!({"role":"user","content":"run gh pr view"}),
        json!({"role":"assistant","content":[{"type":"text","text":"I will run it."}]}),
    ];
    let (_, external_tools, _) = tool_configuration(&initial, None, None);
    let bash_dynamic_name = external_tools
        .iter()
        .find_map(|(dynamic, original)| (original == "Bash").then_some(dynamic.clone()))
        .expect("initial subagent session exposes Bash");
    let initial_signature = bridge.intern_signature(format!(
        "{}\0{}",
        bridge.request_model(&initial),
        crate::anthropic::content::request_signature(&initial, None, None)
            .expect("initial request signature")
    ));
    let mut retained = session_for_model("main", &initial_signature, initial.messages.clone());
    let retained_session = Arc::get_mut(&mut retained).expect("session is not yet shared");
    retained_session.external_tool_names = external_tools;
    retained_session.client_user_id = Some("continued-subagent".to_owned());
    bridge.sessions.lock().await.push(Arc::clone(&retained));

    let mut continued = initial.clone();
    continued.tools.clear();
    continued
        .messages
        .push(json!({"role":"user","content":"now run gh pr view again"}));
    let continued_signature = bridge.intern_signature(format!(
        "{}\0{}",
        bridge.request_model(&continued),
        crate::anthropic::content::request_signature(&continued, None, None)
            .expect("tool-less continuation signature")
    ));

    let selected = bridge
        .select_session(&continued, continued_signature, None, None, &[])
        .await
        .expect("tool-less subagent continuation reuses the routed thread");
    assert!(Arc::ptr_eq(&selected.session, &retained));
    assert_eq!(selected.existing_len, initial.messages.len());
    assert_eq!(
        selected.session.external_tool_names.get(&bash_dynamic_name),
        Some(&"Bash".to_owned())
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
async fn prepare_turn_replaces_a_context_limited_provider_thread() {
    enable_warning_logs();
    let root = tempfile::tempdir().expect("mock app-server fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("create source home");
    std::fs::write(source.join("auth.json"), "{}").expect("write source auth");
    let trace = root.path().join("turns.jsonl");
    let program = write_mock_program(
        &root,
        &format!(
            "#!/bin/sh\nread initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\nread initial\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"initial\"}}}}}}'\nread replacement\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"thread\":{{\"id\":\"replacement\"}}}}}}'\nwhile read line; do printf '%s\\n' \"$line\" >> '{}'; done\n",
            trace.display()
        ),
    );
    let previous_program = std::env::var_os("CLAUDEX_CODEX_PROGRAM");
    let previous_home = std::env::var_os("CODEX_HOME");
    // SAFETY: the test restores both process-wide overrides before observing results.
    unsafe {
        std::env::set_var("CLAUDEX_CODEX_PROGRAM", &program);
        std::env::set_var("CODEX_HOME", &source);
    }

    let mut route = BackendRoute::new("main", BackendKind::CodexAppServer);
    route.max_context_tokens = Some(100);
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[route]), "main".to_owned());
    let result = bridge
        .prepare_turn(&request(Value::Null, Vec::new()), 100, None)
        .await;

    restore_environment("CLAUDEX_CODEX_PROGRAM", previous_program);
    restore_environment("CODEX_HOME", previous_home);

    let turn = result.expect("preemptive replacement starts a fresh provider thread");
    assert_eq!(turn.session.thread_id, "0:replacement");
    assert_eq!(bridge.sessions.lock().await.len(), 1);
    let trace = mock_trace(&trace, 1).await;
    assert_eq!(trace[0]["method"], "turn/start");
    assert_eq!(trace[0]["params"]["threadId"], "replacement");
}

#[tokio::test]
async fn prepare_turn_recovers_transcript_owned_tool_results_after_session_loss() {
    enable_warning_logs();
    let (_root, app) = mock_app_server(
        "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread start\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"recovered\"}}}'\nwhile read line; do :; done\n",
    )
    .await;
    let bridge = Bridge::new_with_backend(AgentBackend::codex(Arc::clone(&app)), "main".to_owned());
    let mut request = request(Value::Null, Vec::new());
    request.messages = vec![
        json!({
            "role":"assistant",
            "content":[{"type":"tool_use","id":"toolu-recovered","name":"Bash","input":{}}]
        }),
        json!({
            "role":"user",
            "content":[{"type":"tool_result","tool_use_id":"toolu-recovered","content":"done"}]
        }),
    ];

    let turn = bridge
        .prepare_turn(&request, 10, None)
        .await
        .expect("transcript-owned result recovers a session");

    assert_eq!(turn.session.thread_id, "recovered");
    assert_eq!(bridge.sessions.lock().await.len(), 1);
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
async fn detached_background_sessions_are_not_selected_but_route_one_late_result() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let session = session("detached", Vec::new());
    session
        .pending_tools
        .lock()
        .await
        .insert("toolu-late".to_owned(), json!(17));
    bridge.sessions.lock().await.push(Arc::clone(&session));

    bridge.detach_session(&session).await;

    assert!(bridge.sessions.lock().await.is_empty());
    assert_eq!(bridge.detached_sessions.lock().await.len(), 1);
    let result = ToolResult {
        tool_use_id: "toolu-late".to_owned(),
        content_items: vec![json!({"type":"text","text":"late result"})],
        is_error: false,
    };
    let found = bridge
        .find_result_session(std::slice::from_ref(&result))
        .await
        .expect("late result owner remains discoverable");
    assert!(Arc::ptr_eq(&found, &session));

    bridge.finish_detached_session(&session).await;
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(bridge.find_result_session(&[result]).await.is_none());
    // Repeated completion is intentionally harmless when a late notification
    // races the background task's final cleanup.
    bridge.finish_detached_session(&session).await;
}

#[tokio::test]
async fn removes_sessions_for_a_failed_model_backend() {
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: "failed".to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: None,
        model_provider: None,
        model_catalog_json: None,
        max_context_tokens: None,
        max_concurrency: None,
        model_prefixes: Vec::new(),
        acp: Some(AcpLaunch {
            program: "/definitely/missing/claudex-acp".to_owned(),
            arguments: Vec::new(),
        }),
        web_search_mode: WebSearchMode::default(),
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
    let bridge = Bridge::new_with_backend(AgentBackend::codex(Arc::clone(&app)), "main".to_owned());
    let previous = session("shared-signature", Vec::new());
    bridge.sessions.lock().await.push(Arc::clone(&previous));
    let gate = Arc::clone(&previous.gate).lock_owned().await;
    let request = request(Value::Null, Vec::new());
    let initial_events = Arc::new(app.subscribe_thread(&previous.thread_id));
    let (selected, extras, events) = bridge
        .recover_turn_start(
            SelectedSession {
                session: Arc::clone(&previous),
                existing_len: 3,
                recovered: true,
                gate,
            },
            Vec::new(),
            initial_events,
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
    app.dispatch_test_event(json!({
        "method":"item/tool/call",
        "params":{"threadId":"replacement","callId":"replacement-tool"}
    }));
    assert_eq!(
        events.recv().await.unwrap()["params"]["callId"],
        "replacement-tool"
    );
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
    wait_for_app_stop(&app).await;
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
