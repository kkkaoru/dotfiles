use crate::{
    agent_backend::{BackendKind, BackendRoute},
    anthropic::MessagesRequest,
};

#[tokio::test]
async fn multi_tui_preempt_requires_the_owning_claude_session_pool() {
    let root = tempfile::tempdir().expect("codex fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let program = root.path().join("codex");
    std::fs::write(
        &program,
        "#!/bin/sh\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
    )
    .expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let server = AppServer::spawn_with_program("main-model", &program, &source, root.path())
        .await
        .expect("app-server");

    let top = AgentBackend::spawn_routes(&[BackendRoute::new(
        "main-model",
        BackendKind::CodexAppServer,
    )]);
    let scopes = as_session_scoped(&top);
    scopes.insert_scope_for_test(
        "tui-a",
        AgentBackend::routed(vec![(
            "main-model".to_owned(),
            AgentBackend::codex(server),
        )]),
    );
    scopes.insert_scope_for_test(
        "tui-b",
        AgentBackend::routed(vec![(
            "other".to_owned(),
            Arc::new(AgentBackend::Grok(crate::grok_acp::GrokAcp::stopped_for_test())),
        )]),
    );
    let _ = scopes.scope(None);

    let busy = session_with_claude_id("main-model", "tui-a");
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let request = request_with_session("main-model", "tui-a");

    // Owning Codex pool: cancel is Unsupported → preempt returns None without waiting.
    let scoped = top.scope_or_self(Some("tui-a"));
    let decided = tokio::time::timeout(
        Duration::from_millis(150),
        select_matching_session(
            vec![Arc::clone(&busy)],
            &request,
            &Arc::from("signature"),
            &request.messages,
            scoped.as_ref(),
        ),
    )
    .await
    .expect("Codex Unsupported cancel must not wait on the busy gate");
    assert!(
        decided.is_none(),
        "Codex session pool must surface Unsupported and start a fresh thread"
    );

    // Top-level SessionScoped with two named pools falls back to `_anonymous`,
    // which reports Settled without cancelling Codex and then waits on the gate.
    let hung = tokio::time::timeout(
        Duration::from_millis(100),
        select_matching_session(
            vec![Arc::clone(&busy)],
            &request,
            &Arc::from("signature"),
            &request.messages,
            top.as_ref(),
        ),
    )
    .await;
    assert!(
        hung.is_err(),
        "unguarded multi-TUI cancel must not silently settle against `_anonymous`"
    );
    drop(gate);
    top.shutdown().await;
}

fn as_session_scoped(backend: &AgentBackend) -> &crate::agent_backend::SessionScopedBackends {
    let AgentBackend::SessionScoped(scopes) = backend else {
        panic!("expected SessionScoped backends");
    };
    scopes
}

fn request_with_session(model: &str, claude_session_id: &str) -> MessagesRequest {
    MessagesRequest {
        model: model.to_owned(),
        system: json!("system"),
        messages: vec![
            json!({"role":"user","content":"first"}),
            json!({"role":"user","content":"follow-up"}),
        ],
        tools: Vec::new(),
        stream: true,
        output_config: json!({}),
        metadata: json!({
            "user_id":"client",
            "_claudex_transport_identity": {
                "session_id": claude_session_id
            }
        }),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn session_with_claude_id(model: &str, claude_session_id: &str) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(Session {
        thread_id: "0:thread".to_owned(),
        model: model.to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(vec![json!({"role":"user","content":"first"})]),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(Default::default()),
        external_tool_names: HashMap::new(),
        client_user_id: Some("client".to_owned()),
        claude_session_id: Some(claude_session_id.to_owned()),
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    })
}
