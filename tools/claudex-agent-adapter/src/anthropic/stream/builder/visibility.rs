use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use crate::anthropic::{Session, content::estimated_tokens, stream::protocol::StreamSender};

impl SegmentBuilder {
    pub(super) async fn observe_subagent_context(
        &mut self,
        session: &Session,
        current_messages: &[Value],
    ) {
        let transcript = session.transcript.lock().await;
        self.subagent_visibility
            .observe_context(&transcript, current_messages);
    }

    pub(super) async fn report_subagent_action(
        &mut self,
        name: &str,
        input: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(status) = self.subagent_visibility.action_status(name, input) else {
            return Ok(());
        };
        if stream.is_none() {
            return Ok(());
        }
        self.append_subagent_status(&status, stream).await
    }

    pub(super) async fn report_no_subagent_action(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if stream.is_none() || self.external_tool_calls > 0 {
            return Ok(());
        }
        let Some(status) = self.subagent_visibility.no_action_notice() else {
            return Ok(());
        };
        self.append_subagent_status(status, stream).await
    }

    async fn append_subagent_status(
        &mut self,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let separator = if self.has_visible_text() { "\n\n" } else { "" };
        let delta = format!("{separator}{status}");
        self.text_delta(&json!({"params":{"delta":delta}}), stream)
            .await?;
        self.injected_output_tokens = self
            .injected_output_tokens
            .saturating_add(estimated_tokens(&delta));
        Ok(())
    }

    fn has_visible_text(&self) -> bool {
        self.open_text_block
            .as_ref()
            .is_some_and(|(_, text)| !text.is_empty())
            || self.blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("text")
                    && block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
            })
    }
}
