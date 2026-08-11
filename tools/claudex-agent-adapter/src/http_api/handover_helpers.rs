use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::listen_handover::ListenHandover;

use super::{HandoverState, RebindRequest, RetainedProxy};

pub(super) fn retained_targets_advertised(
    retained: &RetainedProxy,
    advertised: Option<&ListenHandover>,
) -> bool {
    advertised.is_some_and(|handover| retained.targets_cached(handover.advertised_addr()))
}

pub(super) fn retain_session_locally(
    state: &HandoverState,
    session_id: Option<&str>,
    current: std::net::SocketAddr,
) -> bool {
    let Some(id) = session_id else {
        return false;
    };
    let Some(retained) = state.retained.as_ref() else {
        return false;
    };
    retained.owns(id) && retained.targets(current)
}

pub(super) async fn rebind_listener(
    State(handover): State<ListenHandover>,
    Json(body): Json<RebindRequest>,
) -> Response {
    let previous = handover.advertised_addr();
    if let Some(listen) = body.listen.as_deref() {
        let Ok(listen) = listen.parse() else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "listen must be a socket address"}})),
            )
                .into_response();
        };
        handover.request_bind(listen);
    } else if body.ephemeral {
        handover.request_ephemeral();
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "ephemeral or listen is required"}})),
        )
            .into_response();
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let current = handover.advertised_addr();
        if current != previous {
            return Json(json!({"listen": current.to_string()})).into_response();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(json!({"error": {"message": "listener did not rebind in time"}})),
    )
        .into_response()
}
