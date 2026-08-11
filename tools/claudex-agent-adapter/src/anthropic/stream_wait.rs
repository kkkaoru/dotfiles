use std::{ops::ControlFlow, sync::Arc};

use anyhow::Result;
use serde_json::Value;
use tokio::time::sleep;

use super::super::{Bridge, Session};
use super::{
    SegmentBuilder, StreamEventState, StreamSender, StreamTurn, StreamWaitInput, StreamWaitResult,
    finish_stream_event_state, stream_activity_delays, stream_provider_failure,
};

impl Bridge {
    pub(super) async fn wait_for_stream_segment(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        current_messages: &[Value],
        system: &Value,
        sender: &StreamSender,
        builder: SegmentBuilder,
    ) -> Result<StreamTurn> {
        let (initial_activity_delay, activity_interval) =
            stream_activity_delays(builder.is_subagent);
        self.wait_for_stream_segment_with_interval(StreamWaitInput {
            session,
            events,
            current_messages,
            system,
            sender,
            builder,
            activity_interval,
            initial_activity_delay,
        })
        .await
    }

    // This event loop owns the live SegmentBuilder across keepalive, completion,
    // disconnect, and context-window transitions.
    pub(super) async fn wait_for_stream_segment_with_interval(
        &self,
        input: StreamWaitInput<'_>,
    ) -> Result<StreamTurn> {
        let StreamWaitInput {
            session,
            events,
            current_messages,
            system,
            sender,
            mut builder,
            activity_interval,
            initial_activity_delay,
        } = input;
        // Emit keepalive content during silence to avoid timeout while preserving
        // visible progress semantics during active output.
        let mut activity_deadline = Box::pin(sleep(initial_activity_delay));
        let mut sse = Some(sender);
        loop {
            let wait = match self
                .take_stream_wait(
                    session,
                    Arc::clone(&events),
                    &mut sse,
                    &mut builder,
                    activity_interval,
                    &mut activity_deadline,
                )
                .await?
            {
                ControlFlow::Break(turn) => return Ok(turn),
                ControlFlow::Continue(wait) => wait,
            };
            let state = self
                .resolve_stream_wait(
                    wait,
                    session,
                    sender,
                    current_messages,
                    system,
                    &mut builder,
                )
                .await?;
            match finish_stream_event_state(state, builder) {
                ControlFlow::Break(turn) => return Ok(turn),
                ControlFlow::Continue(next) => builder = next,
            }
        }
    }

    pub(super) async fn take_stream_wait(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        sse: &mut Option<&StreamSender>,
        builder: &mut SegmentBuilder,
        activity_interval: std::time::Duration,
        activity_deadline: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
    ) -> Result<ControlFlow<StreamTurn, StreamWaitResult>> {
        match self
            .wait_for_stream_event(
                session,
                events,
                sse,
                builder,
                activity_interval,
                activity_deadline,
            )
            .await
        {
            Ok(wait) => Ok(ControlFlow::Continue(wait)),
            Err(error) if stream_provider_failure(&error, self, session, builder) => {
                Ok(ControlFlow::Break(StreamTurn::ProviderFailure { error }))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn resolve_stream_wait(
        &self,
        wait: StreamWaitResult,
        session: &Arc<Session>,
        sender: &StreamSender,
        current_messages: &[Value],
        system: &Value,
        builder: &mut SegmentBuilder,
    ) -> Result<StreamEventState> {
        match wait {
            StreamWaitResult::Done(turn) => Ok(StreamEventState::Done(turn)),
            StreamWaitResult::NoEvent => Ok(StreamEventState::Continue),
            StreamWaitResult::Event(event) => {
                self.consume_stream_event(
                    session,
                    sender,
                    current_messages,
                    system,
                    &event,
                    builder,
                )
                .await
            }
        }
    }
}
