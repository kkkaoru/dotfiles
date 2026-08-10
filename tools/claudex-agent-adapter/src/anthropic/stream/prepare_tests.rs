use crate::agent_backend::AgentBackend;
use crate::anthropic::Bridge;

#[tokio::test]
async fn acquire_prepared_permit_returns_none_when_no_ticket() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut request = crate::anthropic::MessagesRequest {
        model: "main".to_owned(),
        system: serde_json::json!("test"),
        messages: vec![],
        tools: vec![],
        stream: false,
        output_config: serde_json::json!({}),
        metadata: serde_json::json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let mut effort = None;

    let result = bridge
        .acquire_prepared_permit(&mut request, &mut effort, None, false)
        .await
        .expect("no-ticket acquire");
    assert!(result.is_none(), "no ticket should yield None permit");
}

#[tokio::test]
async fn acquire_prepared_permit_succeeds_for_a_free_ticket() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let ticket = bridge
        .model_concurrency
        .ticket("test-model", Some(1))
        .expect("ticket");
    let mut request = crate::anthropic::MessagesRequest {
        model: "test-model".to_owned(),
        system: serde_json::json!("test"),
        messages: vec![],
        tools: vec![],
        stream: false,
        output_config: serde_json::json!({}),
        metadata: serde_json::json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    let mut effort = None;

    let result = bridge
        .acquire_prepared_permit(&mut request, &mut effort, Some(ticket), false)
        .await
        .expect("ticket acquire");
    assert!(result.is_some(), "free ticket should yield a permit");
}
