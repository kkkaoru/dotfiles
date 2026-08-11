use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{Bridge, MessagesRequest, request_routing};

pub(super) async fn dispatch_routed_messages(
    bridge: &Arc<Bridge>,
    request: MessagesRequest,
    effort: Option<String>,
    is_subagent: bool,
    tools_were_provided: bool,
    run_in_background: bool,
    route: request_routing::RouteDecision,
) -> Result<Response<Body>> {
    if route == request_routing::RouteDecision::Subscription {
        bridge
            .subscription_messages_with_auth_failover(
                request,
                effort,
                is_subagent,
                tools_were_provided,
            )
            .await
    } else {
        bridge
            .provider_messages_with_usage_limit_failover(
                request,
                effort,
                is_subagent,
                tools_were_provided,
                run_in_background,
            )
            .await
    }
}

pub(super) fn log_provider_turn_end(
    bridge: &Bridge,
    response: &Result<Response<Body>>,
    request_model: &str,
    elapsed: std::time::Duration,
) {
    let duration_ms = elapsed.as_millis();
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            tracing::info!(
                target: "claudex.provider",
                log_event = "provider_turn_end",
                status,
                duration_ms,
                outcome = "response_ready",
                "provider turn response is ready"
            );
        }
        Err(error) => {
            bridge.note_provider_exhaustion(error, Some(request_model));
            tracing::error!(
                target: "claudex.provider",
                log_event = "provider_turn_end",
                duration_ms,
                outcome = "error",
                error = %error,
                "provider turn failed"
            );
        }
    }
}

pub(super) fn acknowledge_internal_notification(request: &MessagesRequest) -> Response<Body> {
    tracing::debug!("acknowledging an internal SubAgent notification without provider turn");
    tracing::info!(
        target: "claudex.provider",
        log_event = "provider_turn_skipped",
        reason = "internal_notification",
        "provider turn skipped for an internal notification"
    );
    super::internal_notification::acknowledge(request)
}

pub(super) fn log_native_background_handoff() {
    tracing::info!(
        target: "claudex.provider",
        log_event = "provider_turn_skipped",
        reason = "native_background_handoff",
        "provider turn skipped after native background handoff"
    );
}
