//! Cline 3.x ACP session/new includes a boolean config option. Without
//! `unstable_boolean_config`, agent-client-protocol rejects the payload as
//! `failed to deserialize response` and every Cline SubAgent dies at session/new.

use agent_client_protocol as acp;

#[test]
fn deserializes_cline_session_new_result_with_boolean_config() {
    let raw = include_str!("fixtures/cline_session_new_result.json");
    let parsed: acp::NewSessionResponse =
        serde_json::from_str(raw).expect("cline session/new result should deserialize");
    assert!(!parsed.session_id.0.is_empty());
    let options = parsed
        .config_options
        .as_ref()
        .expect("cline advertises configOptions");
    assert!(
        options
            .iter()
            .any(|option| option.id.0.as_ref() == "auto_approve"),
        "expected auto_approve boolean option"
    );
}
