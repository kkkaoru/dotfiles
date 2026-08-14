use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{SubscriptionStream, result_output_tokens};
use crate::anthropic::{
    subscription::{subscription_result_text, validate_subscription_result_for_model},
    subscription_frames::{send_text_delta, send_text_finish, send_text_start, send_tool_finish},
};

impl SubscriptionStream {
    pub(super) async fn finish(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
    ) -> Result<()> {
        if self.saw_result {
            return Ok(());
        }
        validate_subscription_result_for_model(result, None)?;
        self.activity.close(sender).await?;
        let deferred_notice = self.activity.take_deferred_text();
        if self.saw_tool_use {
            return self
                .finish_tool_use(sender, result, deferred_notice.as_deref())
                .await;
        }
        if self.blocked_subagent {
            return self
                .finish_blocked_subagent(sender, result, deferred_notice.as_deref())
                .await;
        }
        let include_result_text = !self.text_started;
        if include_result_text || self.text_closed {
            self.open_result_text(sender, result, include_result_text)
                .await?;
        }
        let visibility_tokens = self.report_no_subagent_action(sender).await?;
        if !self.text_closed {
            send_text_finish(
                sender,
                self.next_index.saturating_sub(1),
                result_output_tokens(result).saturating_add(visibility_tokens),
            )
            .await?;
            self.text_closed = true;
        }
        self.saw_result = true;
        Ok(())
    }

    async fn finish_tool_use(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
        deferred_notice: Option<&str>,
    ) -> Result<()> {
        if let Some(notice) = deferred_notice {
            self.open_deferred_notice(sender, notice).await?;
        }
        self.close_text(sender).await?;
        send_tool_finish(sender, result_output_tokens(result)).await?;
        self.saw_result = true;
        Ok(())
    }

    async fn finish_blocked_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
        deferred_notice: Option<&str>,
    ) -> Result<()> {
        if let Some(notice) = deferred_notice {
            self.open_deferred_notice(sender, notice).await?;
        }
        if !self.text_closed {
            send_text_finish(
                sender,
                self.next_index.saturating_sub(1),
                result_output_tokens(result),
            )
            .await?;
            self.text_closed = true;
        }
        self.saw_result = true;
        Ok(())
    }

    async fn open_deferred_notice(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        notice: &str,
    ) -> Result<()> {
        self.close_text(sender).await?;
        send_text_start(sender, self.next_index).await?;
        send_text_delta(sender, self.next_index, notice).await?;
        self.text_started = true;
        self.text_closed = false;
        self.next_index += 1;
        Ok(())
    }

    async fn open_result_text(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
        include_result_text: bool,
    ) -> Result<()> {
        send_text_start(sender, self.next_index).await?;
        if include_result_text {
            let text = subscription_result_text(result).unwrap_or_default();
            send_text_delta(sender, self.next_index, &text).await?;
        }
        self.text_started = true;
        self.text_closed = false;
        self.next_index += 1;
        Ok(())
    }
}
