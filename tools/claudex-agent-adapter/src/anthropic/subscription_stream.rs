use std::{convert::Infallible, path::Path, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    body::{Body, Bytes},
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Child,
    sync::mpsc,
};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use super::{
    content::{estimated_tokens, sse},
    stream::send_stream_frame,
    subscription::{
        OutputMode, SubscriptionOptions, acquire_subscription_slot, spawn_subscription,
        subscription_command, validate_subscription_result, write_subscription_prompt,
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
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("valid subscription streaming response")
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
    tokio::time::timeout(options.timeout, consume_subscription_stream(child, sender))
        .await
        .map_err(|_| anyhow!("Claude subscription timed out after {:?}", options.timeout))?
}

struct SubscriptionStream {
    text_started: bool,
    saw_result: bool,
}

async fn consume_subscription_stream(
    mut child: Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
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
        saw_result: false,
    };
    loop {
        tokio::select! {
            () = sender.closed() => return Ok(()),
            line = lines.next_line() => match line? {
                Some(line) => stream.handle_line(sender, &line).await?,
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
    ) -> Result<()> {
        let envelope: Value = serde_json::from_str(line)
            .with_context(|| format!("Claude subscription emitted invalid stream JSON: {line}"))?;
        match envelope.get("type").and_then(Value::as_str) {
            Some("stream_event") => self.forward_text_delta(sender, &envelope).await,
            Some("result") => self.finish(sender, &envelope).await,
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
        if !self.text_started {
            send_text_start(sender).await?;
            self.text_started = true;
        }
        send_text_delta(sender, text).await
    }

    async fn finish(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        result: &Value,
    ) -> Result<()> {
        validate_subscription_result(result)?;
        if !self.text_started {
            send_text_start(sender).await?;
            let text = result
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default();
            send_text_delta(sender, text).await?;
            self.text_started = true;
        }
        send_text_finish(sender, result_output_tokens(result)).await?;
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

async fn send_text_start(sender: &mpsc::Sender<Result<Bytes, Infallible>>) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_start", || {
        json!({
            "type":"content_block_start","index":0,
            "content_block":{"type":"text","text":""}
        })
    })
    .await
}

async fn send_text_delta(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    text: &str,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta","index":0,
            "delta":{"type":"text_delta","text":text}
        })
    })
    .await
}

async fn send_text_finish(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    output_tokens: u64,
) -> Result<()> {
    for (event, frame) in [
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
            }),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ] {
        send_stream_frame(Some(sender), event, || frame).await?;
    }
    Ok(())
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
