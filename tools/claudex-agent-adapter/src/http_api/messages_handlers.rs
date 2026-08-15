use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use serde_json::{Value, json};

use crate::{
    anthropic::{Bridge, MessagesRequest, RequestIdentity, error_response},
    subagent_policy, working_directory,
};

pub(super) async fn authorize(
    State(expected): State<Option<String>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response<axum::body::Body>, StatusCode> {
    if expected
        .as_deref()
        .is_none_or(|token| has_token(&headers, token))
    {
        return Ok(next.run(request).await);
    }
    Err(StatusCode::UNAUTHORIZED)
}

pub(crate) fn has_token(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("x-api-key")
        .is_some_and(|value| value.as_bytes() == expected.as_bytes())
        || headers
            .get("authorization")
            .is_some_and(|value| value.as_bytes() == format!("Bearer {expected}").as_bytes())
}

pub(super) async fn messages(
    State(bridge): State<Arc<Bridge>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<axum::body::Body> {
    let (mut request, tools_were_provided) = match decode_messages_request(body) {
        Ok(request) => request,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let identity = match request_identity(&headers) {
        Ok(identity) => identity,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    request.working_directory = request_working_directory(&headers);
    if let Err(error) = attach_provider_origin(&headers, &mut request) {
        return error_response(StatusCode::BAD_REQUEST, error);
    }
    let mut disabled_subagent_models = match subagent_policy::active_models() {
        Ok(models) => models,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    match subagent_policy::request_models(&headers) {
        Ok(models) => disabled_subagent_models.extend(models),
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    }
    request.disabled_subagent_models = disabled_subagent_models;
    attach_denylist_warning(&mut request);
    bridge
        .messages_with_identity(request, identity, tools_were_provided)
        .await
        .unwrap_or_else(|error| error_response(StatusCode::BAD_GATEWAY, error))
}

pub(crate) const PROVIDER_ORIGIN_HEADER: &str = "x-claudex-origin";
pub(crate) const PI_PROVIDER_ORIGIN: &str = "pi-provider";
pub(crate) const PI_PROVIDER_ORIGIN_METADATA: &str = "_claudex_pi_provider_origin";

pub(crate) fn attach_provider_origin(
    headers: &HeaderMap,
    request: &mut MessagesRequest,
) -> anyhow::Result<()> {
    let Some(value) = headers.get(PROVIDER_ORIGIN_HEADER) else {
        return Ok(());
    };
    anyhow::ensure!(
        value.as_bytes() == PI_PROVIDER_ORIGIN.as_bytes(),
        "{PROVIDER_ORIGIN_HEADER} must be `{PI_PROVIDER_ORIGIN}`"
    );
    let Value::Object(metadata) = &mut request.metadata else {
        request.metadata = json!({(PI_PROVIDER_ORIGIN_METADATA): true});
        return Ok(());
    };
    metadata.insert(PI_PROVIDER_ORIGIN_METADATA.to_owned(), Value::Bool(true));
    Ok(())
}

fn attach_denylist_warning(request: &mut MessagesRequest) {
    let Some(warning) = subagent_policy::denylist_load_warning() else {
        return;
    };
    match request.metadata {
        Value::Object(ref mut object) => {
            object.insert("_claudex_denylist_load_error".into(), json!(warning));
        }
        _ => {
            request.metadata = json!({ "_claudex_denylist_load_error": warning });
        }
    }
}

pub(crate) fn decode_messages_request(body: Value) -> anyhow::Result<(MessagesRequest, bool)> {
    let tools_were_provided = body
        .as_object()
        .is_some_and(|object| object.contains_key("tools"));
    Ok((serde_json::from_value(body)?, tools_were_provided))
}

pub(crate) const CLAUDE_CODE_SESSION_ID_HEADER: &str = "x-claude-code-session-id";
pub(crate) const CLAUDE_CODE_AGENT_ID_HEADER: &str = "x-claude-code-agent-id";
pub(crate) const CLAUDE_CODE_PARENT_AGENT_ID_HEADER: &str = "x-claude-code-parent-agent-id";
pub(crate) const MAX_CLAUDE_CODE_ID_BYTES: usize = 256;

pub(crate) fn request_identity(headers: &HeaderMap) -> anyhow::Result<RequestIdentity> {
    Ok(RequestIdentity::new(
        request_identity_header(headers, CLAUDE_CODE_SESSION_ID_HEADER)?,
        request_identity_header(headers, CLAUDE_CODE_AGENT_ID_HEADER)?,
        request_identity_header(headers, CLAUDE_CODE_PARENT_AGENT_ID_HEADER)?,
    ))
}

pub(crate) fn request_identity_header(
    headers: &HeaderMap,
    name: &str,
) -> anyhow::Result<Option<String>> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| anyhow::anyhow!("Claude Code identity header `{name}` is not valid UTF-8"))?
        .trim();
    anyhow::ensure!(
        !value.is_empty(),
        "Claude Code identity header `{name}` must not be empty"
    );
    anyhow::ensure!(
        value.len() <= MAX_CLAUDE_CODE_ID_BYTES,
        "Claude Code identity header `{name}` exceeds {MAX_CLAUDE_CODE_ID_BYTES} bytes"
    );
    Ok(Some(value.to_owned()))
}

pub(crate) fn request_working_directory(headers: &HeaderMap) -> Option<std::path::PathBuf> {
    let path = headers
        .get(working_directory::HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .and_then(working_directory::decode)?
        .canonicalize()
        .ok()?;
    path.is_dir().then_some(path)
}

pub(super) async fn count_tokens_handler(
    State(bridge): State<Arc<Bridge>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    let (request, tools_were_provided) = match decode_messages_request(body) {
        Ok(request) => request,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let identity = match request_identity(&headers) {
        Ok(identity) => identity,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let input_tokens = bridge.count_tokens_with_identity(request, &identity, tools_were_provided);
    Json(json!({ "input_tokens": input_tokens })).into_response()
}
