use std::{convert::Infallible, path::Path, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
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
        subscription_command, take_subscription_stdin, validate_subscription_result,
        write_subscription_prompt,
    },
    subscription_activity::SubscriptionActivity,
    subscription_frames::{
        send_block_stop, send_subscription_error, send_text_delta, send_text_finish,
        send_text_start, send_tool_finish,
    },
};

mod tool_collection;

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
    tokio::time::timeout(timeout, async {
        let (prompt_result, stream_result) = tokio::join!(
            write_subscription_prompt(stdin, prompt),
            consume_subscription_stream_with_options(child, sender, &options),
        );
        stream_result?;
        prompt_result.context("failed to write Claude subscription prompt")
    })
    .await
    .map_err(|_| anyhow!("Claude subscription timed out after {timeout:?}"))?
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
    child: Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) -> Result<()> {
    consume_subscription_stream_with_options(
        child,
        sender,
        &SubscriptionOptions::internal(
            Arc::new(tokio::sync::Semaphore::new(1)),
            std::time::Duration::from_secs(1),
        ),
    )
    .await
}

async fn consume_subscription_stream_with_options(
    mut child: Child,
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
    let mut initial_activity = true;
    loop {
        // Prefer child output lines over keepalives so heartbeats never jump
        // ahead of already-buffered stream-json events (scrambled UI order).
        tokio::select! {
            biased;
            () = sender.closed() => return Ok(()),
            line = lines.next_line() => match line? {
                Some(line) => {
                    stream.handle_line(sender, &line).await?;
                    // Real output postpones the idle status timer.
                    activity_deadline.as_mut().reset(
                        tokio::time::Instant::now() + ACTIVITY_KEEPALIVE_INTERVAL,
                    );
                    initial_activity = false;
                }
                None => break,
            },
            () = &mut activity_deadline => {
                if !initial_activity || !stream.text_started {
                    stream.activity_keepalive(sender).await?;
                }
                initial_activity = false;
                activity_deadline.as_mut().reset(
                    tokio::time::Instant::now() + ACTIVITY_KEEPALIVE_INTERVAL,
                );
            }
        }
    }
    stream.activity.close(sender).await?;
    validate_stream_exit(&mut child, stderr_task, stream.saw_result).await
}

async fn validate_stream_exit(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    saw_result: bool,
) -> Result<()> {
    let status = child.wait().await?;
    let stderr = stderr_task.await.context("Claude stderr task failed")??;
    if !status.success() {
        bail!(
            "Claude subscription exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    if !saw_result {
        bail!("Claude subscription stream ended without a result event");
    }
    Ok(())
}

async fn read_stderr(mut stderr: tokio::process::ChildStderr) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stderr.read_to_end(&mut output).await?;
    Ok(output)
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
        super::agent_effort::validate_routed_agent_arguments(
            name,
            input,
            &context.user_messages,
        )?;
        let (intent, public) = super::agent_effort::prepare_arguments_for_user(
            name,
            id,
            input,
            &context.user_messages,
        );
        if let Some(intent) = intent.as_ref() {
            context.agent_efforts.record_from_user_messages(
                context.client_user_id.as_deref(),
                name,
                id.to_owned(),
                &context.parent_model,
                intent,
                &context.user_messages,
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
