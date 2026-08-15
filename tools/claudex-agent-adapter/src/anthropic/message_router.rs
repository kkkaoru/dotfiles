use std::{sync::Arc, time::Instant};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{
    Bridge, MessagesRequest, RequestIdentity, internal_notification, message_router_dispatch,
    pasted_text, request_routing, token_count, trace_request,
};

impl Bridge {
    pub async fn messages(self: &Arc<Self>, request: MessagesRequest) -> Result<Response<Body>> {
        let tools_were_provided = !request.tools.is_empty();
        self.messages_inner(request, tools_were_provided).await
    }

    /// Process a native Claude Code request with its transport identity.
    ///
    /// The identity is attached after body deserialization so a caller cannot
    /// spoof it through `metadata`. It remains available to context retries and
    /// every central SubAgent classifier for the lifetime of the request.
    pub async fn messages_with_identity(
        self: &Arc<Self>,
        mut request: MessagesRequest,
        identity: RequestIdentity,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        let session_id = identity.session_id();
        let agent_id = identity.agent_id();
        let parent_agent_id = identity.parent_agent_id();
        tracing::debug!(
            session_id,
            agent_id,
            parent_agent_id,
            "received Claude Code transport identity"
        );
        self.tool_schemas
            .restore_or_remember(&identity, &mut request, tools_were_provided);
        identity.attach(&mut request);
        self.messages_inner(request, tools_were_provided).await
    }

    pub fn count_tokens_with_identity(
        &self,
        mut request: MessagesRequest,
        identity: &RequestIdentity,
        tools_were_provided: bool,
    ) -> usize {
        self.tool_schemas
            .restore_or_remember(identity, &mut request, tools_were_provided);
        token_count(&request)
    }

    fn apply_turn_preflights(
        self: &Arc<Self>,
        request: &mut MessagesRequest,
        route: request_routing::RouteDecision,
        effort: &mut Option<String>,
        is_subagent: bool,
    ) -> Result<request_routing::RouteDecision> {
        let route = self.apply_usage_limit_preflight(request, route, effort, is_subagent)?;
        let route = self.rewrite_exhausted_subagent_request(request, route, effort, is_subagent)?;
        let route = self.apply_concurrency_preflight(request, route, effort, is_subagent);
        Ok(self.apply_subscription_auth_preflight(request, route, effort))
    }

    fn reject_pi_provider_recursion(&self, request: &MessagesRequest) -> Result<()> {
        let from_pi = request
            .metadata
            .get(crate::http_api::messages_handlers::PI_PROVIDER_ORIGIN_METADATA)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let routes_to_pi = self.app.backend_kind_for_model(&request.model)
            == Some(crate::agent_backend::BackendKind::PiGateway);
        if from_pi && routes_to_pi {
            anyhow::bail!("Pi provider recursion rejected model `{}`", request.model);
        }
        Ok(())
    }

    async fn messages_inner(
        self: &Arc<Self>,
        mut request: MessagesRequest,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        pasted_text::expand_markers(&mut request);
        self.subagent_reuse.observe_and_restore(&mut request);
        self.schedule_idle_session_sweep();
        self.agent_efforts
            .retire_terminal_task_notifications(&request);
        if internal_notification::is_internal_notification_request(&request) {
            return Ok(message_router_dispatch::acknowledge_internal_notification(
                &request,
            ));
        }
        internal_notification::remove_from_transcript(&mut request);
        trace_request(&request);
        if let Some(response) = self.async_agent_launch_handoff(&request).await {
            message_router_dispatch::log_native_background_handoff();
            return Ok(response);
        }
        let intent = self
            .subagent_tool_continuation(&request)
            .await
            .unwrap_or_else(|| self.agent_efforts.take(&request));
        let is_subagent = intent.is_subagent;
        let route = request_routing::resolve_request_model_with_origin(
            &mut request,
            &self.model,
            intent.model_override,
            request_routing::RouteOrigin::new(
                is_subagent,
                intent.matched,
                intent.model_is_inherited,
            ),
            |model| {
                self.app.supports_model(model) || (self.legacy_main_route && model == self.model)
            },
            |model| self.model_catalog.matches(model),
        )?;
        let mut effort = self.resolve_request_effort(&request, intent.effort);
        tracing::debug!(
            request_model = %request.model,
            request_effort = ?effort,
            is_subagent,
            ?route,
            "resolved request routing"
        );
        let route = self.apply_turn_preflights(&mut request, route, &mut effort, is_subagent)?;
        self.reject_pi_provider_recursion(&request)?;
        let request_model = request.model.clone();
        let turn_started = Instant::now();
        tracing::info!(
            target: "claudex.provider",
            log_event = "provider_turn_start",
            request_model = %request.model,
            request_stream = request.stream,
            request_effort = ?effort,
            is_subagent,
            route = ?route,
            "provider turn started"
        );
        let response = message_router_dispatch::dispatch_routed_messages(
            self,
            request,
            effort,
            is_subagent,
            tools_were_provided,
            intent.run_in_background,
            route,
        )
        .await;
        message_router_dispatch::log_provider_turn_end(
            self,
            &response,
            &request_model,
            turn_started.elapsed(),
        );
        response
    }
}

#[cfg(test)]
#[path = "message_router_extra_tests.rs"]
mod extra_tests;

#[cfg(test)]
#[path = "message_router_tests.rs"]
mod tests;
