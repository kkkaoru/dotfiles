use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::SegmentBuilder;
use super::external_tool::ExternalToolContext;

impl SegmentBuilder {
    pub(super) async fn reject_disabled_subagent(
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

    pub(super) async fn reject_exhausted_subagent(
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

    pub(super) async fn reject_stale_task_output(
        &self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        arguments: &Value,
        request_id: Value,
    ) -> Result<bool> {
        if !crate::anthropic::task_ids::is_task_output_tool_name(original_name) {
            return Ok(false);
        }
        let live_ids =
            crate::anthropic::subagent_reuse::live_agent_task_ids(context.current_messages);
        let Some(notice) =
            crate::anthropic::task_ids::stale_task_output_notice(arguments, &live_ids)
        else {
            return Ok(false);
        };
        tracing::info!(
            task_id = crate::anthropic::task_ids::task_output_id(arguments),
            live = live_ids.len(),
            "skipping TaskOutput for unknown Agent task id"
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
            .context("failed to reject a stale TaskOutput provider tool")?;
        Ok(true)
    }

    pub(super) async fn reject_unroutable_subagent(
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
