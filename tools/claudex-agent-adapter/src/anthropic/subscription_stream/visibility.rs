use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;

impl SubscriptionStream {
    pub(super) async fn report_subagent_action(
        &self,
        _sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        _name: &str,
        _input: &Value,
    ) -> Result<()> {
        // Native Claude Code owns the visible Agent/Task status panel. Keep
        // the subscription response protocol free of adapter-only narration.
        Ok(())
    }

    pub(super) async fn report_no_subagent_action(
        &self,
        _sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<u64> {
        Ok(0)
    }
}
