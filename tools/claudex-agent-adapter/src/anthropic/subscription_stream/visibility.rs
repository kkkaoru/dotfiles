use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::{
    content::estimated_tokens,
    subagent_visibility::SubagentVisibility,
    subscription_frames::{send_block_stop, send_text_delta, send_text_start},
};

impl SubscriptionStream {
    pub(super) async fn report_subagent_action(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        input: &Value,
    ) -> Result<()> {
        let Some(status) = self.visibility().action_status(name, input) else {
            return Ok(());
        };
        let index = self.next_index;
        send_text_start(sender, index).await?;
        send_text_delta(sender, index, &status).await?;
        send_block_stop(sender, index).await?;
        self.next_index += 1;
        Ok(())
    }

    pub(super) async fn report_no_subagent_action(
        &self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<u64> {
        let Some(status) = self.visibility().no_action_notice() else {
            return Ok(0);
        };
        let status = format!("\n\n{status}");
        send_text_delta(sender, self.next_index.saturating_sub(1), &status).await?;
        Ok(estimated_tokens(&status))
    }

    fn visibility(&self) -> SubagentVisibility {
        let mut visibility = SubagentVisibility::default();
        if let Some(context) = self.tool_context.as_ref() {
            visibility.observe_context(&[], &context.user_messages);
        }
        visibility
    }
}
