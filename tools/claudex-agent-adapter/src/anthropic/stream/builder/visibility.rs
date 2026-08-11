use anyhow::Result;
use serde_json::Value;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::StreamSender;

impl SegmentBuilder {
    pub(super) async fn report_subagent_action(
        &self,
        _name: &str,
        _input: &Value,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Claude Code renders Agent/Task lifecycle in its native task panel.
        // Do not inject adapter-owned status text into the assistant response:
        // it is not part of Claude's protocol and can be mistaken for user
        // content while background notifications are being delivered.
        Ok(())
    }

    pub(super) async fn report_no_subagent_action(
        &self,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn does_not_inject_synthetic_subagent_status() {
        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);

        let builder = SegmentBuilder::new(1);
        builder
            .report_subagent_action("Agent", &json!({"description":"research"}), None)
            .await
            .expect("non-stream action status");
        builder
            .report_subagent_action("SendMessage", &json!({"to":"worker-1"}), Some(&sender))
            .await
            .expect("stream action");
        assert!(builder.open_text_block.is_none());

        let mut with_open_text = SegmentBuilder::new(1);
        with_open_text.open_text_block = Some((0, "answer".to_owned()));
        with_open_text
            .report_subagent_action("Task", &json!({"description":"next"}), Some(&sender))
            .await
            .expect("open text action");
        assert_eq!(
            with_open_text.open_text_block,
            Some((0, "answer".to_owned()))
        );

        let mut with_committed_text = SegmentBuilder::new(1);
        with_committed_text
            .blocks
            .push(json!({"type":"text","text":"answer"}));
        with_committed_text
            .report_subagent_action("Task", &json!({"description":"next"}), Some(&sender))
            .await
            .expect("committed text action");
        assert!(with_committed_text.open_text_block.is_none());

        let no_action = SegmentBuilder::new(1);
        no_action
            .report_no_subagent_action(Some(&sender))
            .await
            .expect("no-action check");
        assert!(no_action.open_text_block.is_none());
        drop(sender);
        let _drained = receiver.try_recv();
    }
}
