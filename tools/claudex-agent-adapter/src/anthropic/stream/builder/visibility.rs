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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn reports_follow_up_actions_with_and_without_existing_text() {
        let current = vec![
            json!({
                "role":"assistant",
                "content":[{"type":"tool_use","name":"Agent","input":{}}]
            }),
            json!({"role":"user","content":"continue"}),
        ];
        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);

        let mut builder = SegmentBuilder::new(1);
        builder
            .subagent_visibility
            .observe_context(&current, &current);
        builder
            .report_subagent_action("Agent", &json!({"description":"research"}), None)
            .await
            .expect("non-stream action status");
        builder
            .report_subagent_action("SendMessage", &json!({"to":"worker-1"}), Some(&sender))
            .await
            .expect("stream action status");
        assert!(
            builder
                .open_text_block
                .as_ref()
                .is_some_and(|(_, text)| text.contains("SendMessage reuse emitted"))
        );

        let mut with_open_text = SegmentBuilder::new(1);
        with_open_text
            .subagent_visibility
            .observe_context(&current, &current);
        with_open_text.open_text_block = Some((0, "answer".to_owned()));
        with_open_text
            .report_subagent_action("Task", &json!({"description":"next"}), Some(&sender))
            .await
            .expect("open text action status");

        let mut with_committed_text = SegmentBuilder::new(1);
        with_committed_text
            .subagent_visibility
            .observe_context(&current, &current);
        with_committed_text
            .blocks
            .push(json!({"type":"text","text":"answer"}));
        with_committed_text
            .report_subagent_action("Task", &json!({"description":"next"}), Some(&sender))
            .await
            .expect("committed text action status");

        let mut no_action = SegmentBuilder::new(1);
        no_action
            .subagent_visibility
            .observe_context(&current, &current);
        no_action
            .report_no_subagent_action(Some(&sender))
            .await
            .expect("no-action status");
        assert!(
            no_action
                .open_text_block
                .as_ref()
                .is_some_and(|(_, text)| text.contains("no Agent/Task launch"))
        );
        drop(sender);
        while receiver.recv().await.is_some() {}
    }
}
