use std::{pin::Pin, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use tokio::time::Sleep;

use super::{
    Bridge, SegmentBuilder, Session,
    control::refresh_activity_keepalive,
    protocol::StreamSender,
    sanitize::is_visible_activity_event,
    types::{
        StreamWaitResult, fail_if_subagent_provider_silent, reset_activity_deadline,
    },
};
use crate::anthropic::stream_batch::{NextEvent, next_event};

impl Bridge {
    pub(super) async fn wait_for_stream_event(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        sse: &mut Option<&StreamSender>,
        builder: &mut SegmentBuilder,
        activity_interval: Duration,
        activity_deadline: &mut Pin<Box<Sleep>>,
    ) -> Result<StreamWaitResult> {
        let next = if let Some(sender) = *sse {
            tokio::select! {
                biased;
                () = sender.closed() => {
                    *sse = None;
                    if builder.is_subagent {
                        return Ok(self.subagent_sse_closed(session, events, builder).await);
                    }
                    return Ok(StreamWaitResult::Done(Box::new(
                        self.disconnect_stream(session, events).await,
                    )));
                }
                next = next_event(&events, builder.has_external_tool_calls()) => next,
                () = &mut *activity_deadline => {
                    fail_if_subagent_provider_silent(builder)?;
                    refresh_activity_keepalive(
                        builder,
                        Some(sender),
                        activity_deadline.as_mut(),
                        activity_interval,
                    )
                    .await?;
                    return Ok(StreamWaitResult::NoEvent);
                }
            }
        } else {
            tokio::select! {
                biased;
                next = next_event(&events, builder.has_external_tool_calls()) => next,
                () = &mut *activity_deadline => {
                    fail_if_subagent_provider_silent(builder)?;
                    refresh_activity_keepalive(
                        builder,
                        None,
                        activity_deadline.as_mut(),
                        activity_interval,
                    )
                    .await?;
                    return Ok(StreamWaitResult::NoEvent);
                }
            }
        };
        match next {
            NextEvent::Event(event) => {
                reset_activity_deadline(&event, activity_deadline, activity_interval);
                if is_visible_activity_event(&event) {
                    builder.note_visible_provider_activity();
                }
                Ok(StreamWaitResult::Event(event))
            }
            NextEvent::ExternalBatchReady => Ok(StreamWaitResult::Done(Box::new(
                self.external_batch_segment(session, events, builder, *sse)
                    .await?,
            ))),
            NextEvent::Closed => bail!("app-server event stream closed"),
        }
    }
}
