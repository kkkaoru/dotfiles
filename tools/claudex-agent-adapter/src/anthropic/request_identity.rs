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
