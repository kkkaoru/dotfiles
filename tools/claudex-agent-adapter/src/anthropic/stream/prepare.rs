use std::{future::Future, time::Duration};

use anyhow::Result;
use tokio::time::{Instant, sleep};

use axum::body::Bytes;
use serde_json::json;

use super::super::model_concurrency::Ticket;
use super::super::{MessagesRequest, content::sse};
use super::{SegmentBuilder, StreamSender, message_start};

mod drive;
#[cfg(test)]
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
    if !is_subagent || crate::command_code_acp::is_command_code_model(model) {
        return None;
    }
    // Never paint visible launch prose into the first thinking block.
    // Claude Code 2.1 collapses that tip to "Wandering…", and later ▶ Bash /
    // keepalive chrome appended to the same (or follow-on) thinking stream
    // stays hidden for ACP SubAgents even while providerTool events fire.
    // ZWSP priming in `prime_subagent_sse` is enough to keep the SSE channel.
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
    // Claude Code drops SSE after message_start unless a thinking block lands in
    // the same first flush. Main turns also sit blank during ACP session/new +
    // permit acquire; paint immediately for every provider hop through claudex.
    //
    // SubAgents must prime with ZWSP only (same as Command Code). A visible
    // "SubAgent starting… (effort=high)" tip made CC 2.1 collapse the whole
    // thinking block to Wandering, so Cursor ACP ▶ Bash never appeared live
    // even though providerTool events kept firing.
    // First-flush thinking start only. ZWSP / Preparing… must not open a body.
    let _ = is_subagent;
    let _ = sender.try_send(Ok(Bytes::from(subagent_thinking_prime_start())));
    true
}

fn subagent_thinking_prime_start() -> String {
    sse(
        "content_block_start",
        json!({
            "type":"content_block_start",
            "index":0,
            "content_block":{"type":"thinking","thinking":"","signature":""}
        }),
    )
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
                // As above, activity keepalives only emit best-effort frames.
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
