use std::{convert::Infallible, path::Path, path::PathBuf, sync::Arc};

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

use super::{
    content::{estimated_tokens, sse},
    stream::{send_stream_frame, streaming_sse_response},
    subscription::{
        OutputMode, SubscriptionOptions, acquire_subscription_slot, spawn_subscription,
        subscription_command, validate_subscription_result, write_subscription_prompt,
    },
    subscription_frames::{
        assistant_output_tokens, mapped_tool_name, send_block_stop, send_text_delta,
        send_text_finish, send_text_start, send_tool_block, send_tool_finish,
    },
};

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
    write_subscription_prompt(&mut child, prompt).await?;
    let timeout = options.timeout;
    tokio::time::timeout(
        timeout,
        consume_subscription_stream_with_options(child, sender, &options),
    )
    .await
    .map_err(|_| anyhow!("Claude subscription timed out after {timeout:?}"))?
}

struct SubscriptionStream {
    text_started: bool,
    text_closed: bool,
    saw_result: bool,
    next_index: usize,
    tools: Vec<String>,
    tool_context: Option<super::subscription::SubscriptionToolContext>,
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
        saw_result: false,
        next_index: 0,
        tools: options.tools.clone(),
        tool_context: options.tool_context.clone(),
    };
    loop {
        tokio::select! {
            () = sender.closed() => return Ok(()),
            line = lines.next_line() => match line? {
                Some(line) if stream.handle_line(sender, &line).await? => {
                    let _ = child.kill().await;
                    stderr_task.abort();
                    return Ok(());
                }
                Some(_) => {},
                None => break,
            }
        }
    }
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
    ) -> Result<bool> {
        let envelope: Value = serde_json::from_str(line)
            .with_context(|| format!("Claude subscription emitted invalid stream JSON: {line}"))?;
        match envelope.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                self.forward_text_delta(sender, &envelope).await?;
                Ok(false)
            }
            Some("assistant") => self.forward_tool_uses(sender, &envelope).await,
            Some("result") => {
                self.finish(sender, &envelope).await?;
                Ok(false)
            }
            _ => Ok(false),
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
        if !self.text_started {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.next_index += 1;
        }
        send_text_delta(sender, text).await
    }

    async fn forward_tool_uses(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        if envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null())
        {
            return Ok(false);
        }
        let Some(content) = envelope
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return Ok(false);
        };
        let tool_uses = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect::<Vec<_>>();
        if tool_uses.is_empty() {
            return Ok(false);
        }
        self.close_text(sender).await?;
        for block in tool_uses {
            self.forward_tool_use(sender, block).await?;
        }
        send_tool_finish(sender, assistant_output_tokens(envelope)).await?;
        self.saw_result = true;
        Ok(true)
    }

    async fn forward_tool_use(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        block: &Value,
    ) -> Result<()> {
        let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
        let emitted_name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = mapped_tool_name(emitted_name, &self.tools);
        if id.is_empty() || name.is_empty() {
            bail!("Claude subscription emitted a tool call without an ID or name");
        }
        let input = block
            .get("input")
            .filter(|input| input.is_object())
            .cloned()
            .context("Claude subscription emitted non-object tool input")?;
        let public_input = self.prepare_tool_input(name, id, &input);
        send_tool_block(sender, self.next_index, id, name, public_input).await?;
        self.next_index += 1;
        Ok(())
    }

    fn prepare_tool_input(&self, name: &str, id: &str, input: &Value) -> Value {
        if !super::agent_effort::is_agent_tool(name) {
            return input.clone();
        }
        let Some(context) = &self.tool_context else {
            return input.clone();
        };
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
        public
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
        if !self.text_started {
            send_text_start(sender, self.next_index).await?;
            let text = result
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default();
            send_text_delta(sender, text).await?;
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
}

pub(super) fn result_output_tokens(result: &Value) -> u64 {
    result
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            estimated_tokens(
                result
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        })
}

async fn send_subscription_error(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    error: anyhow::Error,
) {
    let _ = send_stream_frame(Some(sender), "error", || {
        json!({
            "type":"error",
            "error":{"type":"api_error","message":format!("{error:#}")}
        })
    })
    .await;
}

#[cfg(test)]
#[path = "subscription_stream_tests.rs"]
mod tests;
