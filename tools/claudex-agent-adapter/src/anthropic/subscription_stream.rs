use anyhow::Result;
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use std::{
    collections::HashSet, convert::Infallible, path::Path, path::PathBuf, sync::Arc, time::Duration,
};
use tokio::sync::mpsc;
// Align with ACP stream.rs: sub-second first paint while the child boots.
pub(super) const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_millis(250);
pub(super) const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
/// Mirrors the ACP SubAgent delay: keep the SubAgent panel lit while a
/// subscription child process boots instead of freezing on Nucleating.
pub(super) const SUBAGENT_INITIAL_ACTIVITY_DELAY: Duration = Duration::from_millis(100);
use super::{
    stream::streaming_sse_response,
    subscription::{
        OutputMode, SubscriptionOptions, acquire_subscription_slot, subscription_command,
        with_transient_retries, write_subscription_prompt,
    },
    subscription_activity::SubscriptionActivity,
    subscription_frames::send_subscription_error,
};

mod consume;
mod consume_fanout;
mod consume_finish;
mod finish;
mod handle;
mod handle_route;
mod handle_thinking;
mod launch_prep;
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
    /// Remapped index + optional signature for a live Claude thinking block.
    thinking_open: Option<(usize, Option<String>)>,
    saw_tool_use: bool,
    launch_fanout_open: bool,
    launch_fanout_deadline: Option<tokio::time::Instant>,
    seen_tool_ids: HashSet<String>,
    blocked_subagent: bool,
    saw_result: bool,
    next_index: usize,
    tools: Vec<String>,
    tool_context: Option<super::subscription::SubscriptionToolContext>,
    activity: SubscriptionActivity,
}

#[path = "subscription_stream_state.rs"]
mod state;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    include!("subscription_stream_tests.rs");
    include!("subscription_stream_thinking_tests.rs");
}
