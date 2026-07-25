use std::{collections::HashMap, sync::Arc, time::Instant};

use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use super::{
    candidate_length, codex_tool_name, dynamic_tool, is_better_length, owns_tool_result,
    reserve_matching_session, thread_start_params, tool_configuration,
    transcript_owns_tool_results,
};
use crate::anthropic::{
    MessagesRequest, Session, subscription_request::subscription_request_prompt,
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
        claudex_collaborator_model: None,
    }
}

fn session(signature: &str, transcript: Vec<Value>) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main-model".to_owned(),
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

#[test]
fn configures_external_and_internal_tools_without_duplicates() {
    let tools = vec![
        json!({"name":"Read","description":"read","input_schema":{"type":"object"}}),
        json!({"description":"missing name"}),
    ];
    let configured = tool_configuration(
        &request(json!("system"), tools),
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
    let configured = tool_configuration(&request(Value::Null, explicit), None, Some("ignored"));
    assert_eq!(configured.0.len(), 1);
    assert!(configured.2.is_empty());
}

#[test]
fn builds_thread_configuration_for_empty_and_team_system_prompts() {
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
    assert!(
        developer
            .contains("never infer from it that Claude Code or its SubAgent tasks are read-only")
    );
    assert!(developer.contains("do not copy restrictions from an unrelated earlier task"));
    assert!(
        developer.contains("preserve that authority in SubAgent prompts"),
        "implementation authority must propagate to SubAgents"
    );
    assert!(
        developer.contains("unless they are explicitly active for the current task"),
        "explicit current-task restrictions must remain supported"
    );
    assert!(developer.contains("Omit the SubAgent name field for ordinary SubAgents"));
    assert!(developer.contains("only when the active user explicitly supplies that teammate name"));
    assert!(developer.contains("every Agent or Task launch, including a nested launch"));
    assert!(developer.contains("exact claudex_model and claudex_effort"));
    assert!(developer.contains("never use generic claude or blindly inherit"));

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
    assert!(is_better_length(None, 1));
    assert!(is_better_length(Some(1), 2));
    assert!(!is_better_length(Some(2), 1));
}
