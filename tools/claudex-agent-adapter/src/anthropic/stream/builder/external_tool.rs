use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{SegmentBuilder, ToolCall};
use crate::anthropic::stream::protocol::{StreamSender, send_tool_use};
use crate::anthropic::{Bridge, Session, retention::record_pending_tool};

#[derive(Clone, Copy)]
pub(super) struct ExternalToolContext<'a> {
    pub(super) bridge: &'a Bridge,
    pub(super) session: &'a Session,
    pub(super) current_messages: &'a [Value],
    pub(super) system: &'a Value,
    pub(super) stream: Option<&'a StreamSender>,
}

pub(super) fn requested_external_tool_name<'a>(
    names: &'a HashMap<String, String>,
    provider_name: &str,
) -> Option<&'a str> {
    names.get(provider_name).map(String::as_str).or_else(|| {
        names
            .values()
            .find(|name| name.as_str() == provider_name)
            .map(String::as_str)
    })
}

pub(super) async fn reject_unrequested_tool(
    bridge: &Bridge,
    session: &Session,
    call: ToolCall,
) -> Result<()> {
    tracing::warn!(
        provider_tool_name = %call.name,
        "rejected a provider tool that Claude Code did not supply"
    );
    let (text, success) = unrequested_tool_reply(&call.name);
    bridge
        .app
        .respond_for_model(
            &session.model,
            call.request_id,
            json!({
                "contentItems":[{"type":"inputText","text":text}],
                "success":success
            }),
        )
        .await
        .context("failed to reject an unrequested provider tool")
}

pub(in crate::anthropic) fn unrequested_tool_reply(name: &str) -> (String, bool) {
    if crate::anthropic::session::is_main_session_only_tool(name) {
        return (
            "Built-in advisor() is main-session only and was not executed. Continue the delegated task without it. Do not retry advisor(), and do not launch models listed in disabled_subagent_models.".to_owned(),
            true,
        );
    }
    (
        format!("Tool `{name}` was not supplied by Claude Code and was not executed."),
        false,
    )
}

impl SegmentBuilder {
    pub(super) async fn external_tool_call(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        call: ToolCall,
    ) -> Result<()> {
        let request_id = call.request_id.clone();
        let mut arguments = call.arguments;
        if crate::anthropic::agent_effort::is_agent_tool(original_name) {
            crate::anthropic::agent_routing::hydrate_routing_fields_from_context(
                &mut arguments,
                context.current_messages,
                context.system,
                context.bridge.model_catalog(),
            );
            crate::anthropic::agent_routing::hydrate_standard_agent_to_parent(
                &mut arguments,
                &context.session.model,
            );
            context.bridge.rewrite_exhausted_agent_launch_with_quota(
                &mut arguments,
                context.current_messages,
                context.system,
            );
        }
        if self
            .reject_disabled_subagent(context, original_name, &arguments, request_id.clone())
            .await?
        {
            return Ok(());
        }
        if self
            .reject_exhausted_subagent(context, original_name, &arguments, request_id.clone())
            .await?
        {
            return Ok(());
        }
        if self
            .reject_unroutable_subagent(context, original_name, &arguments, request_id)
            .await?
        {
            return Ok(());
        }
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        let (intent_arguments, claude_arguments) =
            crate::anthropic::agent_effort::prepare_arguments_for_user(
                original_name,
                &tool_use_id,
                &arguments,
                context.current_messages,
                context.system,
            );
        if let Some(arguments) = intent_arguments.as_ref() {
            context.bridge.agent_efforts.record_from_user_messages(
                crate::anthropic::agent_effort::AgentEffortRecord {
                    client_user_id: context.session.client_user_id.as_deref(),
                    tool_name: original_name,
                    tool_use_id: tool_use_id.clone(),
                    parent_model: &context.session.model,
                    arguments,
                    user_messages: context.current_messages,
                    system: context.system,
                },
                Some(context.bridge.model_catalog()),
            );
        }
        tracing::debug!(call_id = %call.call_id, %tool_use_id, "mapped app-server tool call");
        record_pending_tool(
            context.session,
            tool_use_id.clone(),
            call.request_id,
            std::time::Instant::now(),
        )
        .await;
        self.report_subagent_action(original_name, &arguments, context.stream)
            .await?;
        self.close_open_blocks(context.stream).await?;
        let block = json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": original_name,
            "input": claude_arguments
        });
        let index = self.blocks.len();
        send_tool_use(context.stream, index, &block).await?;
        self.blocks.push(block);
        self.external_tool_calls += 1;
        Ok(())
    }

    async fn reject_disabled_subagent(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        arguments: &Value,
        request_id: Value,
    ) -> Result<bool> {
        let Some(model) = crate::anthropic::agent_effort::disabled_subagent_model(
            original_name,
            arguments,
            &context.session.disabled_subagent_models,
        ) else {
            return Ok(false);
        };
        tracing::warn!(
            tool_name = original_name,
            model,
            "blocked a disabled SubAgent before emitting its launch tool call"
        );
        self.close_open_blocks(context.stream).await?;
        let notice =
            format!("SubAgent model `{model}` is disabled by policy and was not launched.");
        context
            .bridge
            .app
            .respond_for_model(
                &context.session.model,
                request_id,
                json!({
                    "contentItems":[{"type":"inputText","text":notice}],
                    "success":false
                }),
            )
            .await
            .context("failed to reject a disabled SubAgent provider tool")?;
        self.text_delta(
            &serde_json::json!({"params":{"delta":notice}}),
            context.stream,
        )
        .await?;
        self.close_open_blocks(context.stream).await?;
        Ok(true)
    }

    async fn reject_exhausted_subagent(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        arguments: &Value,
        request_id: Value,
    ) -> Result<bool> {
        if !crate::anthropic::agent_effort::is_agent_tool(original_name) {
            return Ok(false);
        }
        let Some(model) = crate::anthropic::agent_effort::requested_model(arguments) else {
            return Ok(false);
        };
        if !context.bridge.subagent_provider_is_exhausted(model) {
            return Ok(false);
        }
        tracing::warn!(
            tool_name = original_name,
            model,
            "blocked an exhausted SubAgent before emitting its launch tool call"
        );
        self.close_open_blocks(context.stream).await?;
        let notice = format!(
            "SubAgent model `{model}` is cooling down after a rate/usage/billing limit; pick another selected_workers entry."
        );
        context
            .bridge
            .app
            .respond_for_model(
                &context.session.model,
                request_id,
                json!({
                    "contentItems":[{"type":"inputText","text":notice}],
                    "success":false
                }),
            )
            .await
            .context("failed to reject an exhausted SubAgent provider tool")?;
        self.text_delta(
            &serde_json::json!({"params":{"delta":notice}}),
            context.stream,
        )
        .await?;
        self.close_open_blocks(context.stream).await?;
        Ok(true)
    }

    async fn reject_unroutable_subagent(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        arguments: &Value,
        request_id: Value,
    ) -> Result<bool> {
        if !crate::anthropic::agent_effort::is_agent_tool(original_name) {
            return Ok(false);
        }
        if crate::anthropic::agent_effort::validate_routed_agent_arguments_with_catalog(
            original_name,
            arguments,
            context.current_messages,
            context.system,
            context.bridge.model_catalog(),
        )
        .is_ok()
        {
            return Ok(false);
        }
        tracing::warn!(
            tool_name = original_name,
            subagent_type = ?arguments.get("subagent_type"),
            "blocked unsupported SubAgent launch"
        );
        self.close_open_blocks(context.stream).await?;
        let notice = crate::anthropic::agent_effort::BLOCKED_SUBAGENT_NOTICE;
        context
            .bridge
            .app
            .respond_for_model(
                &context.session.model,
                request_id,
                json!({
                    "contentItems":[{"type":"inputText","text":notice}],
                    "success":false
                }),
            )
            .await
            .context("failed to reject an unroutable SubAgent provider tool")?;
        self.text_delta(
            &serde_json::json!({"params":{"delta":notice}}),
            context.stream,
        )
        .await?;
        self.close_open_blocks(context.stream).await?;
        Ok(true)
    }
}
