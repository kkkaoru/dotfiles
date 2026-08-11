use serde_json::{Map, Value, json};

use super::MessagesRequest;

const METADATA_KEY: &str = "_claudex_transport_identity";

/// Claude Code transport identity supplied by the native `/v1/messages` request.
///
/// These values are deliberately kept outside the serialized Anthropic body. The
/// HTTP headers are authoritative whenever at least one is present; transcript
/// markers are only a compatibility fallback for older Claude Code clients.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestIdentity {
    session_id: Option<String>,
    agent_id: Option<String>,
    parent_agent_id: Option<String>,
}

impl RequestIdentity {
    #[must_use]
    pub fn new(
        session_id: Option<String>,
        agent_id: Option<String>,
        parent_agent_id: Option<String>,
    ) -> Self {
        Self {
            session_id,
            agent_id,
            parent_agent_id,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    #[must_use]
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    #[must_use]
    pub fn parent_agent_id(&self) -> Option<&str> {
        self.parent_agent_id.as_deref()
    }

    pub(super) fn attach(self, request: &mut MessagesRequest) {
        if !request.metadata.is_object() {
            request.metadata = Value::Object(Map::new());
        }
        let metadata = request.metadata.as_object_mut().expect("metadata object");
        metadata.remove(METADATA_KEY);
        if self.authoritative_is_subagent().is_none() {
            return;
        }
        metadata.insert(
            METADATA_KEY.to_owned(),
            json!({
                "session_id": self.session_id,
                "agent_id": self.agent_id,
                "parent_agent_id": self.parent_agent_id,
            }),
        );
    }

    fn authoritative_is_subagent(&self) -> Option<bool> {
        if self.agent_id.is_some() || self.parent_agent_id.is_some() {
            return Some(true);
        }
        self.session_id.as_ref().map(|_| false)
    }
}

pub(super) fn authoritative_is_subagent(request: &MessagesRequest) -> Option<bool> {
    let identity = request.metadata.get(METADATA_KEY)?.as_object()?;
    if identity.get("agent_id").is_some_and(nonempty_string)
        || identity.get("parent_agent_id").is_some_and(nonempty_string)
    {
        return Some(true);
    }
    identity
        .get("session_id")
        .is_some_and(nonempty_string)
        .then_some(false)
}

fn nonempty_string(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
}

/// Claude Code conversation id for isolating SubAgents across concurrent TUIs.
///
/// Prefer the `x-claude-code-session-id` transport header. Fall back to a
/// `user_id` JSON blob's `session_id` when older clients omit the header.
pub(crate) fn claude_session_id(request: &MessagesRequest) -> Option<String> {
    transport_session_id(request).or_else(|| user_id_session_id(request))
}

pub(crate) fn request_agent_id(request: &MessagesRequest) -> Option<String> {
    request
        .metadata
        .pointer("/_claudex_transport_identity/agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn transport_session_id(request: &MessagesRequest) -> Option<String> {
    request
        .metadata
        .pointer("/_claudex_transport_identity/session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn user_id_session_id(request: &MessagesRequest) -> Option<String> {
    let raw = request.metadata.get("user_id")?.as_str()?;
    serde_json::from_str::<Value>(raw)
        .ok()?
        .get("session_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
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
}
