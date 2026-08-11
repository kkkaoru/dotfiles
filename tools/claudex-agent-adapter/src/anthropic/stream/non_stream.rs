use std::ops::ControlFlow;

use anyhow::{Result, bail};
use serde_json::Value;

use super::{Bridge, NextEvent, Segment, SegmentBuilder, Session, StreamSender, next_event};
use crate::anthropic::segment::EMPTY_ACP_END_TURN;

impl Bridge {
    pub(in crate::anthropic) async fn wait_for_segment(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
        input_tokens: u64,
        current_messages: &[Value],
        system: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<Segment> {
        let mut builder = SegmentBuilder::new(input_tokens);
        loop {
            match self
                .wait_for_segment_step(
                    session,
                    events,
                    current_messages,
                    system,
                    stream,
                    &mut builder,
                )
                .await?
            {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(segment) => return break_segment_or_empty(segment),
            }
        }
    }

    async fn wait_for_segment_step(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
        current_messages: &[Value],
        system: &Value,
        stream: Option<&StreamSender>,
        builder: &mut SegmentBuilder,
    ) -> Result<ControlFlow<Segment>> {
        let event = match next_event(events, builder.has_external_tool_calls()).await {
            NextEvent::Event(event) => event,
            NextEvent::ExternalBatchReady => {
                return Ok(ControlFlow::Break(builder.finish(stream).await?));
            }
            NextEvent::Closed => bail!("app-server event stream closed"),
        };
        match builder
            .handle_event(self, session, current_messages, system, &event, stream)
            .await?
        {
            ControlFlow::Continue(()) => Ok(ControlFlow::Continue(())),
            ControlFlow::Break(()) => Ok(ControlFlow::Break(builder.finish(stream).await?)),
        }
    }
}

fn break_segment_or_empty(segment: Segment) -> Result<Segment> {
    if segment.is_empty_end_turn() {
        bail!("{EMPTY_ACP_END_TURN}");
    }
    Ok(segment)
}
