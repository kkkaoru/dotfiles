use std::{convert::Infallible, path::Path, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
    sync::mpsc,
};
use uuid::Uuid;

// Align with the main provider stream: status only after real silence (~30s).
const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_secs(30);
const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
use super::{
    content::sse,
    stream::streaming_sse_response,
    subscription::{
        OutputMode, SubscriptionOptions, acquire_subscription_slot, spawn_subscription,
        subscription_command, take_subscription_stdin, terminate_subscription,
        validate_subscription_result, write_subscription_prompt,
    },
    subscription_activity::SubscriptionActivity,
    subscription_frames::{
        send_block_stop, send_subscription_error, send_text_delta, send_text_finish,
        send_text_start, send_tool_finish,
    },
};

mod lifecycle;
mod tool_collection;

use lifecycle::{
    read_stderr, terminate_after_stream_failure, terminate_closed_stream, validate_stream_exit,
};

pub(super) use super::subscription_frames::result_output_tokens;
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

pub(super) fn subscription_start_frame(model: &str, input_tokens: u64) -> String {
    sse(
        "message_start",
        json!({
            "type":"message_start",
            "message":{
                "id":format!("msg_{}", Uuid::new_v4().simple()),
                "type":"message","role":"assistant","model":model,
                "content":[],"stop_reason":null,"stop_sequence":null,
                "usage":{"input_tokens":input_tokens,"output_tokens":0}
            }
        }),
    )
}

async fn run_subscription_stream(
    sender: mpsc::Sender<Result<Bytes, Infallible>>,
    program: PathBuf,
    model: String,
    prompt: String,
    options: SubscriptionOptions,
) {
    let result = stream_subscription_model(&sender, &program, &model, &prompt, options).await;
    if let Err(error) = result {
        tracing::warn!(%model, error = ?error, "Claude subscription stream failed");
        send_subscription_error(&sender, error).await;
    }
}

async fn stream_subscription_model(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    program: &Path,
    model: &str,
    prompt: &str,
    options: SubscriptionOptions,
) -> Result<()> {
    let _permit = acquire_subscription_slot(Arc::clone(&options.slots), options.timeout).await?;
    let mut command = subscription_command(program, model, &options, OutputMode::StreamJson);
    let mut child = spawn_subscription(&mut command, model)?;
    let stdin = take_subscription_stdin(&mut child)?;
    // Defer stdin errors so an early process exit can report its status and stderr.
    let timeout = options.timeout;
    let result = tokio::time::timeout(timeout, async {
        let (prompt_result, stream_result) = tokio::join!(
            write_subscription_prompt(stdin, prompt),
            consume_subscription_stream_with_options(&mut child, sender, &options),
        );
        stream_result?;
        prompt_result.context("failed to write Claude subscription prompt")
    })
    .await
    .map_err(|_| anyhow!("Claude subscription timed out after {timeout:?}"));
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => terminate_after_stream_failure(&mut child, error).await,
        Err(error) => {
            terminate_subscription(&mut child).await?;
            Err(error)
        }
    }
}

struct SubscriptionStream {
    text_started: bool,
    text_closed: bool,
    saw_tool_use: bool,
    saw_result: bool,
    next_index: usize,
    tools: Vec<String>,
    tool_context: Option<super::subscription::SubscriptionToolContext>,
    activity: SubscriptionActivity,
}

#[cfg(test)]
async fn consume_subscription_stream(
    mut child: Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) -> Result<()> {
    let result = consume_subscription_stream_with_options(
        &mut child,
        sender,
        &SubscriptionOptions::internal(
            Arc::new(tokio::sync::Semaphore::new(1)),
            std::time::Duration::from_secs(1),
        ),
    )
    .await;
    if let Err(error) = result {
        return terminate_after_stream_failure(&mut child, error).await;
    }
    Ok(())
}

async fn consume_subscription_stream_with_options(
    child: &mut Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
) -> Result<()> {
    let stdout = child
        .stdout
        .take()
        .context("Claude subscription stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Claude subscription stderr is unavailable")?;
    let stderr_task = tokio::spawn(read_stderr(stderr));
    let mut lines = BufReader::new(stdout).lines();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: options.tools.clone(),
        tool_context: options.tool_context.clone(),
        activity: SubscriptionActivity::default(),
    };
    let mut activity_deadline = Box::pin(tokio::time::sleep(INITIAL_ACTIVITY_DELAY));
    loop {
        // Prefer child output lines over keepalives so heartbeats never jump
        // ahead of already-buffered stream-json events (scrambled UI order).
        tokio::select! {
            biased;
            () = sender.closed() => return terminate_closed_stream(child, stderr_task).await,
            line = lines.next_line() => match line? {
                Some(line) => {
                    stream.handle_line(sender, &line).await?;
                    // Real output postpones the idle status timer.
                    activity_deadline.as_mut().reset(
                        tokio::time::Instant::now() + ACTIVITY_KEEPALIVE_INTERVAL,
                    );
                }
                None => break,
            },
            () = &mut activity_deadline => {
                stream.activity_keepalive(sender).await?;
                activity_deadline.as_mut().reset(
                    tokio::time::Instant::now() + ACTIVITY_KEEPALIVE_INTERVAL,
                );
            }
        }
    }
    stream.activity.close(sender).await?;
    validate_stream_exit(child, stderr_task, stream.saw_result).await
}

impl SubscriptionStream {
    async fn handle_line(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        line: &str,
    ) -> Result<()> {
        let envelope: Value = serde_json::from_str(line)
            .with_context(|| format!("Claude subscription emitted invalid stream JSON: {line}"))?;
        match envelope.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                self.forward_text_delta(sender, &envelope).await?;
                Ok(())
            }
            Some("assistant") => self.forward_tool_uses(sender, &envelope).await,
            Some("result") => {
                self.finish(sender, &envelope).await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn forward_text_delta(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<()> {
        if envelope
            .pointer("/event/delta/type")
            .and_then(Value::as_str)
            != Some("text_delta")
        {
            return Ok(());
        }
        let text = envelope
            .pointer("/event/delta/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if text.is_empty() {
            return Ok(());
        }
        if self.saw_tool_use {
            return Ok(());
        }
        self.activity.close(sender).await?;
        if !self.text_started {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.next_index += 1;
        }
        send_text_delta(sender, self.next_index.saturating_sub(1), text).await
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

    async fn finish(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
    ) -> Result<()> {
        validate_subscription_result(result)?;
        self.activity.close(sender).await?;
        if self.saw_tool_use {
            send_tool_finish(sender, result_output_tokens(result)).await?;
            self.saw_result = true;
            return Ok(());
        }
        if !self.text_started {
            send_text_start(sender, self.next_index).await?;
            let text = result
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default();
            send_text_delta(sender, self.next_index, text).await?;
            self.text_started = true;
            self.next_index += 1;
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
            return Ok(());
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
