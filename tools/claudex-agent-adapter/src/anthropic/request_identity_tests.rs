use serde_json::json;

use super::*;
use crate::anthropic::MessagesRequest;

fn request(metadata: Value) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: Vec::new(),
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

#[test]
fn prefers_transport_session_id_over_user_id_blob() {
    let request = request(json!({
        "user_id": r#"{"device_id":"dev","session_id":"from-user"}"#,
        "_claudex_transport_identity":{"session_id":"from-header"}
    }));
    assert_eq!(claude_session_id(&request).as_deref(), Some("from-header"));
}

#[test]
fn falls_back_to_user_id_json_session_id() {
    let request = request(json!({
        "user_id": r#"{"device_id":"dev","session_id":"from-user"}"#
    }));
    assert_eq!(claude_session_id(&request).as_deref(), Some("from-user"));
}

#[test]
fn ignores_plain_user_id_and_empty_ids() {
    assert_eq!(
        claude_session_id(&request(json!({"user_id":"client"}))),
        None
    );
    assert_eq!(
        claude_session_id(&request(json!({
            "_claudex_transport_identity":{"session_id":""}
        }))),
        None
    );
    assert_eq!(claude_session_id(&request(json!({}))), None);
}

#[test]
fn attach_with_no_identity_skips_metadata() {
    let identity = RequestIdentity::new(None, None, None);
    let mut req = request(json!({}));
    identity.attach(&mut req);
    assert_eq!(req.metadata.get(METADATA_KEY), None);
}

#[test]
fn attach_with_session_id_only() {
    let identity = RequestIdentity::new(Some("sess-1".to_owned()), None, None);
    let mut req = request(json!({}));
    identity.attach(&mut req);
    let attached = req.metadata.get(METADATA_KEY).and_then(|v| v.as_object());
    assert!(attached.is_some());
    assert_eq!(
        attached.and_then(|obj| obj.get("session_id").and_then(|v| v.as_str())),
        Some("sess-1")
    );
}

#[test]
fn attach_with_agent_id_marks_subagent() {
    let identity = RequestIdentity::new(None, Some("agent-1".to_owned()), None);
    let mut req = request(json!({}));
    identity.attach(&mut req);
    assert!(req.metadata.get(METADATA_KEY).is_some());
}

#[test]
fn authoritative_is_subagent_with_agent_id() {
    let identity = RequestIdentity::new(None, Some("agent-1".to_owned()), None);
    assert_eq!(identity.authoritative_is_subagent(), Some(true));
}

#[test]
fn authoritative_is_subagent_with_parent_agent_id() {
    let identity = RequestIdentity::new(None, None, Some("parent-1".to_owned()));
    assert_eq!(identity.authoritative_is_subagent(), Some(true));
}

#[test]
fn authoritative_is_subagent_with_session_id_only() {
    let identity = RequestIdentity::new(Some("sess-1".to_owned()), None, None);
    assert_eq!(identity.authoritative_is_subagent(), Some(false));
}

#[test]
fn nonempty_string_accepts_only_non_empty() {
    assert!(nonempty_string(&json!("text")));
    assert!(!nonempty_string(&json!("")));
    assert!(!nonempty_string(&json!(null)));
    assert!(!nonempty_string(&json!(123)));
}
