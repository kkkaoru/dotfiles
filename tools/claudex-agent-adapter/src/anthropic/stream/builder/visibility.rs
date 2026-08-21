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

    pub(super) fn report_no_subagent_action(&mut self) {
        if self.is_subagent
            || self.suppressed_tool_use
            || blocks_contain_subagent_launch(&self.blocks)
        {
            return;
        }
        let text = assistant_text(&self.blocks);
        if self.requires_subagent_launch || contains_unbacked_launch_claim(&text) {
            tracing::warn!(
                "provider SubAgent execution contract lacked an Agent/Task tool call; retrying"
            );
            self.recoverable_empty_output = true;
        }
    }
}

fn blocks_contain_subagent_launch(blocks: &[Value]) -> bool {
    blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("tool_use")
            && block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(crate::anthropic::agent_effort::is_agent_tool)
    })
}

fn assistant_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains_unbacked_launch_claim(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let negated = [
        "起動していません",
        "起動していない",
        "委譲していません",
        "委譲されていません",
        "呼び出しをしていません",
        "起動できません",
        "委譲できません",
        "not launched",
        "not started",
        "not delegated",
        "did not launch",
        "didn't launch",
        "did not delegate",
        "no worker was started",
        "no agent was started",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern));
    if negated {
        return false;
    }
    [
        "委譲しました",
        "起動しました",
        "バックグラウンドで実行中",
        "バックグラウンドで起動中",
        "delegated to",
        "have delegated",
        "has been delegated",
        "worker launched",
        "agent launched",
        "launched the worker",
        "launched the agent",
        "started the worker",
        "started the agent",
        "running in the background",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
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

        let mut no_action = SegmentBuilder::new(1);
        no_action.report_no_subagent_action();
        assert!(no_action.open_text_block.is_none());
        drop(sender);
        let _drained = receiver.try_recv();
    }

    #[test]
    fn detects_only_assertive_unbacked_launch_claims() {
        assert!(contains_unbacked_launch_claim(
            "ARM64 migration was delegated to one worker."
        ));
        assert!(contains_unbacked_launch_claim(
            "ARM64 移行を1ワーカーに委譲しました。バックグラウンドで実行中です。"
        ));
        assert!(!contains_unbacked_launch_claim(
            "いいえ、起動していません。前のターンで「委譲しました」と書いただけでした。"
        ));
        assert!(!contains_unbacked_launch_claim(
            "I did not launch a worker; no agent was started."
        ));
    }
}
