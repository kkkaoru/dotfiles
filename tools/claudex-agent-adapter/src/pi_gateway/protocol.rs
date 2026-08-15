use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub(super) const VERSION: u64 = 1;
pub(super) const ORIGIN: &str = "claudex";

pub(super) fn hello(token: &str) -> Value {
    json!({"version": VERSION, "type": "hello", "token": token})
}

pub(super) fn request(
    id: &str,
    token: &str,
    provider: &str,
    model_id: &str,
    request: &Value,
    effort: Option<&str>,
) -> Result<Value> {
    if provider == "claudex" {
        bail!("Pi gateway recursion rejected provider `claudex`");
    }
    let system = request.get("system").cloned().unwrap_or(Value::Null);
    let messages = request
        .get("messages")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let tools = request.get("tools").cloned().unwrap_or_else(|| json!([]));
    let metadata = request
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut options = json!({"metadata": metadata});
    if let Some(session_id) = request
        .pointer("/metadata/_claudex_transport_identity/session_id")
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
    {
        options["sessionId"] = json!(session_id);
    }
    if let Some(reasoning) = effort.and_then(pi_reasoning) {
        options["reasoning"] = json!(reasoning);
    }
    Ok(json!({
        "version": VERSION,
        "type": "request",
        "id": id,
        "token": token,
        "origin": ORIGIN,
        "provider": provider,
        "modelId": model_id,
        "system": system,
        "messages": messages,
        "tools": tools,
        "options": options
    }))
}

pub(super) fn cancel(id: &str, token: &str) -> Value {
    json!({"version": VERSION, "type": "cancel", "id": id, "token": token})
}

pub(super) fn validate_ready(value: &Value) -> Result<()> {
    validate_version(value)?;
    if value.get("type").and_then(Value::as_str) != Some("ready") {
        bail!("Pi gateway expected ready handshake, received {value}");
    }
    Ok(())
}

pub(super) fn validate_event<'a>(value: &'a Value, request_id: &str) -> Result<&'a str> {
    validate_version(value)?;
    if value.get("id").and_then(Value::as_str) != Some(request_id) {
        bail!("Pi gateway event request id does not match `{request_id}`");
    }
    value
        .get("type")
        .and_then(Value::as_str)
        .context("Pi gateway event omitted type")
}

fn validate_version(value: &Value) -> Result<()> {
    if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
        bail!("Pi gateway protocol version mismatch");
    }
    Ok(())
}

fn pi_reasoning(effort: &str) -> Option<&str> {
    match effort {
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" => Some(effort),
        "max" => Some("xhigh"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_preserves_raw_anthropic_fields_and_rejects_recursion() {
        let raw = json!({
            "system":[{"type":"text","text":"rules"}],
            "messages":[{"role":"user","content":"hello"}],
            "tools":[{"name":"Read","input_schema":{"type":"object"}}],
            "metadata":{
                "user_id":"u",
                "_claudex_transport_identity":{"session_id":"claude-session"}
            }
        });
        let value =
            request("r", "t", "openai-codex", "gpt", &raw, Some("max")).expect("gateway request");
        assert_eq!(value["system"], raw["system"]);
        assert_eq!(value["messages"], raw["messages"]);
        assert_eq!(value["tools"], raw["tools"]);
        assert_eq!(value["options"]["metadata"], raw["metadata"]);
        assert_eq!(value["options"]["sessionId"], "claude-session");
        assert_eq!(value["options"]["reasoning"], "xhigh");
        assert_eq!(value["origin"], ORIGIN);
        assert!(request("r", "t", "claudex", "model", &raw, None).is_err());
    }

    #[test]
    fn request_omits_missing_or_blank_transport_session_ids() {
        for metadata in [
            json!({}),
            json!({"user_id":r#"{"session_id":"fallback"}"#}),
            json!({"_claudex_transport_identity":{"session_id":""}}),
        ] {
            let raw = json!({"metadata":metadata});
            let value =
                request("r", "t", "openai-codex", "gpt", &raw, None).expect("gateway request");
            assert!(value["options"].get("sessionId").is_none());
        }
    }

    #[test]
    fn validates_handshake_version_and_event_correlation() {
        assert!(validate_ready(&json!({"version":1,"type":"ready"})).is_ok());
        assert!(validate_ready(&json!({"version":2,"type":"ready"})).is_err());
        assert_eq!(
            validate_event(&json!({"version":1,"id":"r","type":"done"}), "r").expect("event"),
            "done"
        );
        assert!(validate_event(&json!({"version":1,"id":"other","type":"done"}), "r").is_err());
    }
}
