use std::{sync::Arc, time::Duration};

use super::super::{SegmentBuilder, StreamSender, StreamTurn, protocol::send_stream_graceful_stop};
use super::{StreamDriveOptions, response_timeout};
use crate::anthropic::{
    ActiveTurn, Bridge, model_concurrency::ModelPermit, subagent_timeout::completes_within,
};

impl Bridge {
    #[cfg(test)]
    pub(in crate::anthropic::stream) async fn drive_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
    ) {
        self.drive_subagent_stream(turn, sender, builder, model_permit, false, false)
            .await;
    }

    pub(in crate::anthropic::stream) async fn drive_subagent_stream(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        let timeout = response_timeout(self.subagent_hard_timeout, is_subagent, run_in_background);
        self.drive_subagent_stream_with_timeout(
            turn,
            sender,
            builder,
            StreamDriveOptions {
                model_permit,
                is_subagent,
                run_in_background,
                timeout,
            },
        )
        .await;
    }

    pub(in crate::anthropic::stream) async fn drive_subagent_stream_with_timeout(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        builder: SegmentBuilder,
        options: StreamDriveOptions,
    ) {
        let StreamDriveOptions {
            model_permit,
            is_subagent,
            run_in_background,
            timeout,
        } = options;
        let waited = self
            .wait_for_stream_turn(&turn, &sender, builder, timeout)
            .await;
        let Some(waited) = waited else {
            let timeout = timeout.expect("elapsed stream wait has a configured timeout");
            let error = self.expire_subagent_turn(&turn, timeout).await;
            tracing::warn!(?error, "streaming turn timed out after message_start");
            drop(model_permit);
            send_stream_graceful_stop(&sender).await;
            return;
        };
        self.finish_stream_turn(
            turn,
            sender,
            waited,
            model_permit,
            is_subagent,
            run_in_background,
        )
        .await;
    }

    async fn wait_for_stream_turn(
        &self,
        turn: &ActiveTurn,
        sender: &StreamSender,
        builder: SegmentBuilder,
        timeout: Option<Duration>,
    ) -> Option<anyhow::Result<StreamTurn>> {
        let wait = self.wait_for_stream_segment(
            &turn.session,
            Arc::clone(&turn.events),
            &turn.extras,
            &turn.routing_system,
            sender,
            builder,
        );
        completes_within(timeout, wait).await
    }

    async fn finish_stream_turn(
        self: Arc<Self>,
        turn: ActiveTurn,
        sender: StreamSender,
        waited: anyhow::Result<StreamTurn>,
        model_permit: Option<ModelPermit>,
        is_subagent: bool,
        run_in_background: bool,
    ) {
        match waited {
            Ok(outcome) => {
                self.finish_stream_turn_ok(
                    turn,
                    sender,
                    outcome,
                    model_permit,
                    is_subagent,
                    run_in_background,
                )
                .await;
            }
            Err(error) => {
                self.finish_stream_turn_err(
                    turn,
                    sender,
                    error,
                    model_permit,
                    is_subagent,
                    run_in_background,
                )
                .await;
            }
        }
    }
}

#[path = "run_retry.rs"]
mod retry;
