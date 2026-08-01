use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::Value;
use std::{
    collections::HashSet, convert::Infallible, path::Path, path::PathBuf, sync::Arc, time::Duration,
};
use tokio::sync::mpsc;
// Align with the main provider stream: status only after real silence (~30s).
pub(super) const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_secs(30);
pub(super) const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
use super::{
    stream::streaming_sse_response,
    subscription::{
        OutputMode, SubscriptionOptions, acquire_subscription_slot, subscription_command,
        with_transient_retries, write_subscription_prompt,
    },
    subscription_activity::SubscriptionActivity,
    subscription_frames::{
        send_block_stop, send_subscription_error, send_text_delta, send_text_start,
    },
};

mod consume;
mod finish;
mod lifecycle;
mod post_eof;
mod tool_collection;
mod visibility;
pub(super) use super::subscription_frames::{result_output_tokens, subscription_start_frame};
#[cfg(test)]
use consume::consume_subscription_stream;
use consume::consume_subscription_stream_with_options;
use lifecycle::terminate_after_stream_failure;
pub(super) fn subscription_streaming_response(
    program: PathBuf,
    model: String,
    prompt: String,
    input_tokens: u64,
    options: SubscriptionOptions,
) -> Response<Body> {
    let (sender, receiver) = mpsc::channel(64);
    sender
        .try_send(Ok(Bytes::from(subscription_start_frame(
            &model,
            input_tokens,
        ))))
        .expect("new subscription stream has capacity");
    tokio::spawn(run_subscription_stream(
        sender, program, model, prompt, options,
    ));
    streaming_sse_response(receiver)
}

async fn run_subscription_stream(
    sender: mpsc::Sender<Result<Bytes, Infallible>>,
    program: PathBuf,
    model: String,
    prompt: String,
    options: SubscriptionOptions,
) {
    match with_transient_retries(&model, || {
        stream_subscription_model(&sender, &program, &model, &prompt, &options)
    })
    .await
    {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(%model, error = ?error, "Claude subscription stream failed");
            send_subscription_error(&sender, error).await;
        }
    }
}

async fn stream_subscription_model(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    program: &Path,
    model: &str,
    prompt: &str,
    options: &SubscriptionOptions,
) -> Result<()> {
    let _permit = acquire_subscription_slot(Arc::clone(&options.slots), options.timeout).await?;
    let mut command = subscription_command(program, model, options, OutputMode::StreamJson);
    let (mut child, stdin) = super::subscription::failure::spawn_child(&mut command, model)?;
    let process_group = child.id();
    // Defer stdin errors so an early process exit can report its status and stderr.
    let timeout = options.timeout;
    match tokio::time::timeout(timeout, async {
        let (prompt_result, stream_result) = tokio::join!(
            write_subscription_prompt(stdin, prompt),
            consume_subscription_stream_with_options(&mut child, sender, options, model),
        );
        stream_result?;
        // A disconnected Claude Code response is a caller cancellation. The
        // provider may close stdin while the prompt writer is still flushing;
        // do not turn that expected teardown into a user-visible API error.
        if sender.is_closed() {
            return Ok(());
        }
        super::subscription::failure::local_result(model, "failed to write prompt", prompt_result)
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            terminate_after_stream_failure(
                &mut child,
                process_group,
                options.termination_timeout,
                error,
            )
            .await
        }
        Err(_) => {
            terminate_after_stream_failure(
                &mut child,
                process_group,
                options.termination_timeout,
                super::subscription::failure::timeout_failure(model, timeout),
            )
            .await
        }
    }
}

struct SubscriptionStream {
    text_started: bool,
    text_closed: bool,
    saw_tool_use: bool,
    seen_tool_ids: HashSet<String>,
    blocked_subagent: bool,
    saw_result: bool,
    next_index: usize,
    tools: Vec<String>,
    tool_context: Option<super::subscription::SubscriptionToolContext>,
    activity: SubscriptionActivity,
}
impl SubscriptionStream {
    #[cfg(test)]
    async fn handle_line(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        line: &str,
    ) -> Result<()> {
        if self.saw_result {
            return Ok(());
        }
        let envelope = super::subscription::failure::parse_stream_envelope(None, line)?;
        self.handle_envelope(sender, &envelope).await?;
        Ok(())
    }

    async fn handle_envelope(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        match envelope.get("type").and_then(Value::as_str) {
            Some("stream_event") => self.forward_text_delta(sender, envelope).await,
            Some("assistant") => self.forward_tool_uses(sender, envelope).await,
            Some("result") => {
                self.finish(sender, envelope).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn forward_text_delta(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        if envelope
            .pointer("/event/delta/type")
            .and_then(Value::as_str)
            != Some("text_delta")
        {
            return Ok(false);
        }
        let text = envelope
            .pointer("/event/delta/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if text.is_empty() {
            return Ok(false);
        }
        if self.saw_tool_use || self.blocked_subagent {
            return Ok(false);
        }
        self.activity.close(sender).await?;
        if !self.text_started || self.text_closed {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.text_closed = false;
            self.next_index += 1;
        }
        send_text_delta(sender, self.next_index.saturating_sub(1), text).await?;
        Ok(true)
    }

    fn prepare_tool_input(&self, name: &str, id: &str, input: &Value) -> Result<Value> {
        if !super::agent_effort::is_agent_tool(name) {
            return Ok(input.clone());
        }
        let context = self
            .tool_context
            .as_ref()
            .context("subscription Agent/Task call has no routing context")?;
        let mut routed_input = input.clone();
        super::agent_routing::hydrate_routing_fields_from_context(
            &mut routed_input,
            &context.user_messages,
            &context.system,
            &context.model_catalog,
        );
        super::agent_routing::hydrate_standard_agent_to_parent(
            &mut routed_input,
            &context.parent_model,
        );
        if routed_input.get("claudex_model").is_none() {
            tracing::warn!(
                tool = name,
                subagent_type = ?routed_input.get("subagent_type"),
                native_model = ?routed_input.get("model"),
                "subscription Agent/Task omitted Claudex routing fields"
            );
        }
        super::agent_effort::validate_routed_agent_arguments_with_catalog(
            name,
            &routed_input,
            &context.user_messages,
            &context.system,
            &context.model_catalog,
        )?;
        let (intent, public) = super::agent_effort::prepare_arguments_for_user(
            name,
            id,
            &routed_input,
            &context.user_messages,
            &context.system,
        );
        if let Some(intent) = intent.as_ref() {
            context.agent_efforts.record_from_user_messages(
                super::agent_effort::AgentEffortRecord {
                    client_user_id: context.client_user_id.as_deref(),
                    tool_name: name,
                    tool_use_id: id.to_owned(),
                    parent_model: &context.parent_model,
                    arguments: intent,
                    user_messages: &context.user_messages,
                    system: &context.system,
                },
                Some(&context.model_catalog),
            );
        }
        Ok(public)
    }

    async fn close_text(&mut self, sender: &mpsc::Sender<Result<Bytes, Infallible>>) -> Result<()> {
        if self.text_started && !self.text_closed {
            send_block_stop(sender, self.next_index.saturating_sub(1)).await?;
            self.text_closed = true;
        }
        Ok(())
    }

    async fn activity_keepalive(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        if self.saw_result {
            return Ok(());
        }
        if self.saw_tool_use {
            return self
                .activity
                .keepalive(sender, None, &mut self.next_index)
                .await;
        }
        if self.text_closed {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.text_closed = false;
            self.next_index += 1;
        }
        let text_index = self.text_started.then(|| self.next_index.saturating_sub(1));
        self.activity
            .keepalive(sender, text_index, &mut self.next_index)
            .await
    }
}

#[cfg(test)]
#[path = "subscription_stream_tests.rs"]
mod tests;
