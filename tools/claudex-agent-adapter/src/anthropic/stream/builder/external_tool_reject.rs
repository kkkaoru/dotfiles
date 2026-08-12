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
            .app_for_session(context.session)
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
            .app_for_session(context.session)
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
}

#[path = "external_tool_reject_stale.rs"]
mod stale;
