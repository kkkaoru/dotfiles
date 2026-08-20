use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::SegmentBuilder;
use super::external_tool::ExternalToolContext;
use crate::anthropic::agent_route_validation::BlockedSubagentError;
use crate::anthropic::stream::protocol::{StreamSender, send_stream_frame};

impl SegmentBuilder {
    /// Keep a blocked Agent notice identically visible in the live stream and
    /// committed transcript.  This is an assistant-text outcome, never a
    /// synthetic executable tool call.
    pub(super) async fn emit_blocked_notice(
        &mut self,
        notice: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Provider already stopped for tool_use; we answer with text instead of
        // emitting Claude tool_use. Mark suppression so finish() ends cleanly.
        self.suppressed_tool_use = true;
        self.close_open_blocks(stream).await?;
        self.note_provider_turn_activity();
        let index = self.start_text_block(notice, stream).await?;
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":notice}
            })
        })
        .await?;
        self.close_text_block(stream).await
    }

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
        let notice = BlockedSubagentError::policy_disabled(model).notice();
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
        self.emit_blocked_notice(&notice, context.stream).await?;
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
        let notice = BlockedSubagentError::cooldown(model).notice();
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
        self.emit_blocked_notice(&notice, context.stream).await?;
        self.close_open_blocks(context.stream).await?;
        Ok(true)
    }

    pub(super) async fn reject_duplicate_subagent(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        request_id: Value,
    ) -> Result<()> {
        const NOTICE: &str = "A same-scope SubAgent is already running, so this duplicate launch was not started. Continue with the existing worker.";
        tracing::info!(
            session_id = ?context.session.claude_session_id,
            tool_name = original_name,
            "rejected duplicate provider SubAgent launch"
        );
        context
            .bridge
            .app_for_session(context.session)
            .respond_for_model(
                &context.session.model,
                request_id,
                json!({
                    "contentItems":[{"type":"inputText","text":NOTICE}],
                    "success":false
                }),
            )
            .await
            .context("failed to reject a duplicate provider SubAgent")?;
        self.suppressed_tool_use = true;
        if self.external_tool_calls == 0 {
            self.emit_blocked_notice(NOTICE, context.stream).await?;
            self.close_open_blocks(context.stream).await?;
        }
        Ok(())
    }
}

#[path = "external_tool_reject_stale.rs"]
mod stale;

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use tokio::sync::mpsc;

    use super::SegmentBuilder;

    #[tokio::test]
    async fn blocked_notice_streams_as_committed_text_without_tool_use() {
        let notice = "The requested SubAgent model is disabled by policy, so it was not started. Continue without it.";
        let mut builder = SegmentBuilder::new(0);
        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(4);

        builder
            .emit_blocked_notice(notice, Some(&sender))
            .await
            .expect("blocked notice streams");
        assert_eq!(
            builder.blocks,
            vec![serde_json::json!({"type":"text","text":notice})]
        );

        drop(sender);
        let output = [
            receiver
                .recv()
                .await
                .expect("content start")
                .expect("infallible frame"),
            receiver
                .recv()
                .await
                .expect("text delta")
                .expect("infallible frame"),
            receiver
                .recv()
                .await
                .expect("content stop")
                .expect("infallible frame"),
        ]
        .map(|frame| String::from_utf8_lossy(&frame).into_owned())
        .concat();
        assert!(output.contains(r#""content_block""#));
        assert!(output.contains(r#""type":"text""#));
        assert!(output.contains(r#""type":"text_delta""#));
        assert!(!output.contains(r#""type":"thinking_delta""#));
        assert!(!output.contains(r#""type":"tool_use""#));
    }
}
