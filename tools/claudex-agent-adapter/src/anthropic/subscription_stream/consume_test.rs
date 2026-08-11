use std::convert::Infallible;

use anyhow::{Context, Result};
use axum::body::Bytes;
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::mpsc;

use super::{
    StreamIteration, SubscriptionStream, consume_stream_iteration,
};
use crate::anthropic::subscription::SubscriptionOptions;

impl SubscriptionStream {
    pub(crate) async fn consume_reader_for_test<R>(
        reader: R,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        options: &SubscriptionOptions,
        model: &str,
    ) -> Result<()>
    where
        R: AsyncBufRead + Unpin,
    {
        consume_test_reader(reader, sender, options, model).await
    }
}

async fn consume_test_reader<R>(
    reader: R,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
    model: &str,
) -> Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = reader.lines();
    let mut stream = SubscriptionStream::from_options(options);
    stream
        .start_subagent_activity(sender, options, model)
        .await?;
    let mut pending_result = None;
    let mut activity_deadline = Box::pin(tokio::time::sleep(options.initial_activity_delay));
    loop {
        let iteration = consume_stream_iteration(
            &mut lines,
            sender,
            model,
            &mut stream,
            &mut pending_result,
            &mut activity_deadline,
            options.activity_keepalive_interval,
        )
        .await?;
        match iteration {
            StreamIteration::Continue => {}
            StreamIteration::End => break,
            StreamIteration::SenderClosed => return Ok(()),
            StreamIteration::EndEarly => {
                finish_end_early(sender, &mut stream).await?;
                return Ok(());
            }
        }
    }
    stream.activity.close(sender).await?;
    let result = pending_result.context("test reader ended without a result")?;
    stream.finish(sender, &result).await
}

async fn finish_end_early(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    stream: &mut SubscriptionStream,
) -> Result<()> {
    if stream.blocked_subagent {
        anyhow::bail!("test reader emitted a blocked SubAgent");
    }
    stream.activity.close(sender).await?;
    stream
        .finish(
            sender,
            &json!({"type":"result","subtype":"success","is_error":false,"result":""}),
        )
        .await
}

