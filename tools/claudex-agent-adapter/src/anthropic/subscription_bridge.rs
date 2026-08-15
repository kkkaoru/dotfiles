use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};
use serde_json::{Value, json};

use super::{
    ACTIVITY_KEEPALIVE_INTERVAL, Bridge, DEFAULT_STDERR_DRAIN_GRACE, DEFAULT_TERMINATION_TIMEOUT,
    INITIAL_ACTIVITY_DELAY, MessagesRequest, SUBAGENT_INITIAL_ACTIVITY_DELAY, Segment,
    SubscriptionOptions, SubscriptionToolContext, Usage, WebEvidenceSummary, anthropic_response,
    estimated_tokens, is_compaction_request, request_json_schema, requested_tools_for_request,
    run_subscription_model, subscription_request_cwd, subscription_request_prompt,
    subscription_streaming_response, token_count,
};

impl Bridge {
    pub(in crate::anthropic) async fn subscription_messages(
        &self,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        let options = self.subscription_options(&request, effort, is_subagent, tools_were_provided);
        let prompt = subscription_request_prompt(&request);
        if request.stream {
            return Ok(subscription_streaming_response(
                self.subscription_program.clone(),
                request.model,
                prompt,
                input_tokens,
                options,
            ));
        }
        let text =
            run_subscription_model(&self.subscription_program, &request.model, &prompt, options)
                .await?;
        let segment = Segment {
            blocks: vec![json!({"type":"text", "text":text})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens,
                output_tokens: estimated_tokens(&text),
                ..Usage::default()
            },
            web_evidence: WebEvidenceSummary::default(),
        };
        Ok(anthropic_response(segment, &request.model))
    }

    pub(in crate::anthropic) fn subscription_options(
        &self,
        request: &MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> SubscriptionOptions {
        SubscriptionOptions {
            effort,
            is_subagent,
            tools: requested_tools_for_request(request, !is_subagent),
            disable_tools: (tools_were_provided && request.tools.is_empty())
                || is_compaction_request(request),
            json_schema: request_json_schema(&request.output_config),
            cwd: subscription_request_cwd(request),
            slots: Arc::clone(&self.subscription_slots),
            timeout: self.subscription_timeout,
            initial_activity_delay: if is_subagent {
                SUBAGENT_INITIAL_ACTIVITY_DELAY
            } else {
                INITIAL_ACTIVITY_DELAY
            },
            activity_keepalive_interval: ACTIVITY_KEEPALIVE_INTERVAL,
            stderr_drain_grace: DEFAULT_STDERR_DRAIN_GRACE,
            termination_timeout: DEFAULT_TERMINATION_TIMEOUT,
            tool_context: Some(SubscriptionToolContext {
                agent_efforts: Arc::clone(&self.agent_efforts),
                model_catalog: self.model_catalog.clone(),
                client_user_id: request
                    .metadata
                    .get("user_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parent_model: request.model.clone(),
                user_messages: request.messages.clone(),
                system: request.system.clone(),
                session_id: crate::anthropic::subagent_reuse::session_id(request),
                subagent_reuse: Arc::clone(&self.subagent_reuse),
                auth_cache: self.provider_auth_cache_path(),
                disabled_subagent_models: request.disabled_subagent_models.clone(),
            }),
        }
    }
}
