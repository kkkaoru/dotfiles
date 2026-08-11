use super::{SubscriptionStream, consume_fanout};

impl SubscriptionStream {
    pub(super) fn arm_launch_fanout(&mut self) {
        self.launch_fanout_open = true;
        self.launch_fanout_deadline =
            Some(tokio::time::Instant::now() + consume_fanout::LAUNCH_FANOUT_DRAIN);
    }

    pub(super) fn clear_launch_fanout(&mut self) {
        self.launch_fanout_open = false;
        self.launch_fanout_deadline = None;
    }

    #[cfg(test)]
    pub(super) async fn handle_line(
        &mut self,
        sender: &tokio::sync::mpsc::Sender<
            Result<axum::body::Bytes, std::convert::Infallible>,
        >,
        line: &str,
    ) -> anyhow::Result<()> {
        if self.saw_result {
            return Ok(());
        }
        let envelope = super::super::subscription::failure::parse_stream_envelope(None, line)?;
        self.handle_envelope(sender, &envelope).await?;
        Ok(())
    }
}
