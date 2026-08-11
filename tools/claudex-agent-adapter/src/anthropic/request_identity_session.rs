use serde_json::Value;

use super::MessagesRequest;

pub(super) fn transport_session_id(request: &MessagesRequest) -> Option<String> {
    request
        .metadata
        .pointer("/_claudex_transport_identity/session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

pub(super) fn user_id_session_id(request: &MessagesRequest) -> Option<String> {
    let raw = request.metadata.get("user_id")?.as_str()?;
    serde_json::from_str::<Value>(raw)
        .ok()?
        .get("session_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}
