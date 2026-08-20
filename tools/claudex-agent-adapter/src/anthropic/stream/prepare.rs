use std::{future::Future, time::Duration};

use anyhow::Result;
use tokio::time::{Instant, sleep};

use super::super::MessagesRequest;
use super::super::model_concurrency::Ticket;
use super::{SegmentBuilder, StreamSender, message_start};
use axum::body::Bytes;

mod drive;
#[cfg(test)]
#[allow(unused_imports)]
pub(in crate::anthropic::stream) use drive::prepare_first_activity_delay;

pub(in crate::anthropic) struct PreparedStream {
    pub(in crate::anthropic) request: MessagesRequest,
    pub(in crate::anthropic) input_tokens: u64,
    pub(in crate::anthropic) effort: Option<String>,
    pub(in crate::anthropic) concurrency_ticket: Option<Ticket>,
    pub(in crate::anthropic) is_subagent: bool,
    pub(in crate::anthropic) run_in_background: bool,
    pub(in crate::anthropic) sender: StreamSender,
    pub(in crate::anthropic) primed_thinking: bool,
}

pub(super) fn subagent_start_status(
    is_subagent: bool,
    model: &str,
    _effort: Option<&str>,
) -> Option<String> {
    if !is_subagent {
        return None;
    }
    let _ = model;
    // Never paint visible launch prose into the first thinking block.
    // Claude Code 2.1 collapses that tip to "Wandering…", and later ▶ Bash /
    // keepalive chrome appended to the same (or follow-on) thinking stream
    // stays hidden for ACP SubAgents even while providerTool events fire.
    // `KeepaliveStream` comments keep the SSE channel alive during preparation.
    None
}

pub(super) fn prime_subagent_sse(
    sender: &StreamSender,
    model: &str,
    input_tokens: u64,
    is_subagent: bool,
    _effort: Option<&str>,
) -> bool {
    sender
        .try_send(Ok(Bytes::from(message_start(model, input_tokens))))
        .expect("new streaming response channel has capacity");
    // The response body itself supplies SSE comment keepalives while preparation
    // is quiet. Do not synthesize a thinking block here: a silent Cline turn
    // must stay block-free until real CoT or ACP progress arrives. If the turn
    // still has zero blocks at finish, inject visible assistant text before
    // `end_turn` — an empty close after `message_start` is "No assistant messages found".
    let _ = is_subagent;
    true
}

pub(super) struct PrepareActivityOptions<'a> {
    pub(in crate::anthropic) input_tokens: u64,
    pub(in crate::anthropic) sender: &'a StreamSender,
    pub(in crate::anthropic) initial_status: Option<&'a str>,
    pub(in crate::anthropic) first_delay: Duration,
    pub(in crate::anthropic) interval: Duration,
    pub(in crate::anthropic) is_subagent: bool,
    pub(in crate::anthropic) paint_command_code_progress: bool,
    pub(in crate::anthropic) primed_thinking: bool,
}

pub(super) async fn prepare_with_activity<F, T>(
    prepare: F,
    options: PrepareActivityOptions<'_>,
) -> (Result<Option<T>>, SegmentBuilder)
where
    F: Future<Output = Result<T>>,
{
    let PrepareActivityOptions {
        input_tokens,
        sender,
        initial_status,
        first_delay,
        interval,
        is_subagent,
        paint_command_code_progress,
        primed_thinking,
    } = options;
    let mut builder = SegmentBuilder::new(input_tokens)
        .with_subagent(is_subagent)
        .with_command_code_progress(paint_command_code_progress);
    let mut sse_open = true;
    if primed_thinking {
        builder = builder.with_primed_thinking();
    } else if let Some(status) = initial_status {
        // Stream-frame delivery deliberately absorbs a closed SSE receiver;
        // the status path therefore has no fallible production outcome.
        let _ = builder.subagent_start_status(status, Some(sender)).await;
    }
    let mut deadline = Box::pin(sleep(first_delay));
    tokio::pin!(prepare);
    loop {
        tokio::select! {
            biased;
            () = sender.closed(), if sse_open => {
                if !is_subagent {
                    return (Ok(None), builder);
                }
                // Claude Code often drops SubAgent SSE immediately after
                // message_start. Keep preparing so ACP/cmd is not torn down.
                sse_open = false;
                continue;
            }
            result = &mut prepare => return (result.map(Some), builder),
            () = &mut deadline => {
                // The response body emits comment keepalives; this does not
                // create synthetic content blocks.
                let _ = builder.activity_keepalive(sse_open.then_some(sender)).await;
            },
        };
        deadline.as_mut().reset(Instant::now() + interval);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "prepare_tests.rs"]
mod tests;
