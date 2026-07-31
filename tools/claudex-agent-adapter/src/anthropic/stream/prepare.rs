use std::{future::Future, time::Duration};

use anyhow::Result;
use tokio::time::{Instant, sleep};

use super::super::model_concurrency::Ticket;
use super::{MessagesRequest, SegmentBuilder, StreamSender};

pub(super) struct PreparedStream {
    pub(super) request: MessagesRequest,
    pub(super) input_tokens: u64,
    pub(super) effort: Option<String>,
    pub(super) concurrency_ticket: Option<Ticket>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
    pub(super) sender: StreamSender,
}

pub(super) async fn prepare_with_activity<F, T>(
    prepare: F,
    input_tokens: u64,
    sender: &StreamSender,
    first_delay: Duration,
    interval: Duration,
) -> (Result<Option<T>>, SegmentBuilder)
where
    F: Future<Output = Result<T>>,
{
    let mut builder = SegmentBuilder::new(input_tokens);
    let mut deadline = Box::pin(sleep(first_delay));
    tokio::pin!(prepare);
    loop {
        let result = tokio::select! {
            biased;
            () = sender.closed() => return (Ok(None), builder),
            result = &mut prepare => return (result.map(Some), builder),
            () = &mut deadline => builder.activity_keepalive(Some(sender)).await,
        };
        if let Err(error) = result {
            return (Err(error), builder);
        }
        deadline.as_mut().reset(Instant::now() + interval);
    }
}
