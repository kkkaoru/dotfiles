use std::{convert::Infallible, pin::Pin};

use anyhow::Result;
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::{process::Child, sync::mpsc, time::Sleep};

use super::{SubscriptionStream, lifecycle::validate_stream_exit, post_eof::await_post_eof};
use crate::anthropic::subscription::{SubscriptionOptions, failure};

pub(super) async fn finish_blocked_stream(
    child: &mut Child,
    process_group: Option<u32>,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    mut stream: SubscriptionStream,
    options: &SubscriptionOptions,
) -> Result<()> {
    super::post_eof::cleanup_closed_stream(child, process_group, stderr_task, options).await?;
    stream
        .finish(
            sender,
            &json!({"type":"result", "subtype":"success", "is_error":false, "result":""}),
        )
        .await
}

pub(super) async fn finish_stream(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    mut stream: SubscriptionStream,
    pending_result: Option<Value>,
    context: FinishStreamContext<'_>,
) -> Result<()> {
    let Some(output) = await_post_eof(
        child,
        context.process_group,
        stderr_task,
        context.sender,
        context.options,
        &mut stream,
        context.activity_deadline,
    )
    .await?
    else {
        return Ok(());
    };
    stream.activity.close(context.sender).await?;
    if let Err(error) = validate_stream_exit(
        &output.status,
        &output.stderr,
        pending_result.as_ref(),
        context.model,
    ) {
        return Err(stream_error(&mut stream, context.sender, context.model, error).await);
    }
    let result = pending_result.expect("validated stream result is present");
    if let Err(error) = stream.finish(context.sender, &result).await {
        return Err(stream_error(&mut stream, context.sender, context.model, error).await);
    }
    Ok(())
}

pub(super) struct FinishStreamContext<'a> {
    pub(super) process_group: Option<u32>,
    pub(super) sender: &'a mpsc::Sender<Result<Bytes, Infallible>>,
    pub(super) model: &'a str,
    pub(super) options: &'a SubscriptionOptions,
    pub(super) activity_deadline: &'a mut Pin<Box<Sleep>>,
}

async fn stream_error(
    stream: &mut SubscriptionStream,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    model: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let _ = stream.close_text(sender).await;
    if stream.next_index > 0 {
        failure::after_stream_output(model, error)
    } else {
        error
    }
}
