use std::sync::Arc;

use axum::{body::Body, http::Response};
use tokio::sync::mpsc;

use super::{Bridge, MessagesRequest, Segment, Session, model_concurrency::Ticket};

mod builder;
mod context_retry;
mod context_window;
mod control;
mod disconnect;
mod drive;
mod drive_finish;
mod empty_turn;
mod event_consume;
mod non_stream;
mod prepare;
mod protocol;
mod provider_tool;
mod sanitize;
#[cfg(test)]
mod subagent_live_view;
#[cfg(test)]
mod subagent_progress_models_tests;
mod thinking;
mod thinking_support;
mod tool_call_parser;
mod turn;
mod types;
pub(super) mod usage_limit;
mod wait_event;
pub(in crate::anthropic) use turn::StreamTurn;
pub(super) use types::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    StreamWaitInput, ToolCall, is_provider_stream_closed,
};
use types::{StreamEventState, StreamWaitResult, stream_activity_delays};

use builder::SegmentBuilder;
pub(in crate::anthropic) use control::commit_transcript;
use event_consume::{finish_stream_event_state, stream_provider_failure};
#[cfg(test)]
use prepare::{PrepareActivityOptions, prepare_with_activity};
use prepare::{PreparedStream, prime_subagent_sse};

pub(super) use crate::anthropic::stream_batch::{NextEvent, next_event};
pub(super) use control::{error_flow, turn_flow};
#[cfg(test)]
pub(super) use protocol::tool_use_frames;
use protocol::{StreamSender, sse_response};
pub(super) use protocol::{
    message_start, send_stream_completion, send_stream_frame, streaming_sse_response,
};

impl Bridge {
    pub(super) fn streaming_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        concurrency_ticket: Option<Ticket>,
        is_subagent: bool,
        run_in_background: bool,
    ) -> Response<Body> {
        let (sender, receiver) = mpsc::channel(256);
        let response_model = self.request_model(&request);
        let primed_thinking = prime_subagent_sse(
            &sender,
            &response_model,
            input_tokens,
            is_subagent,
            effort.as_deref(),
        );
        tokio::spawn(
            Arc::clone(self).drive_prepared_subagent_stream(PreparedStream {
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
                run_in_background,
                sender,
                primed_thinking,
            }),
        );
        sse_response(receiver)
    }
}

#[path = "stream_wait.rs"]
mod wait;

#[cfg(test)]
mod tests;
