use super::*;
use serde_json::json;
use std::collections::HashSet;

#[test]
fn serializes_pending_request_id_keys_without_reordering_tools() {
    let keys = request_id_keys(&[
        ("first".to_owned(), json!(41)),
        ("second".to_owned(), json!({"id":"42"})),
    ]);
    assert!(keys.contains("41"));
    assert!(keys.contains(r#"{"id":"42"}"#));
}

#[tokio::test]
async fn rejects_each_request_id_once_and_reports_provider_errors() {
    let app =
        crate::agent_backend::AgentBackend::Pi(crate::pi_gateway::PiGateway::stopped_for_test());
    let mut rejected = HashSet::new();
    assert!(
        reject_disconnected_tool_once(&app, "model", &mut rejected, json!(41))
            .await
            .is_err()
    );
    assert!(
        reject_disconnected_tool_once(&app, "model", &mut rejected, json!(41))
            .await
            .is_ok()
    );
    assert_eq!(rejected.len(), 1);
}
