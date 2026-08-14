use anyhow::Result;
use serde_json::Value;

use super::ThinkingState;
use crate::anthropic::stream::StreamSender;

#[cfg(test)]
fn is_keepalive_item(item_id: &str) -> bool {
    item_id == "claudex_activity_keepalive" || item_id == "claudex_provider_progress"
}

impl ThinkingState {
    /// Synthetic status chrome never becomes a thinking block. The response
    /// wrapper keeps an otherwise quiet SSE connection alive with comments.
    pub(in crate::anthropic::stream) async fn activity_status(
        &self,
        _blocks: &mut Vec<Value>,
        _status: &str,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Status/launch chrome is not model reasoning or provider-visible ACP
        // progress. The response wrapper maintains SSE liveness with comments.
        Ok(())
    }

    pub(in crate::anthropic::stream) fn prime_silent_heartbeat(&self, _blocks: &mut Vec<Value>) {
        // Compatibility hook: priming now reserves no block and emits no wire
        // frame. Real CoT/visible ACP progress opens the first thinking block.
    }

    #[cfg(test)]
    pub(in crate::anthropic::stream) fn holds_live_cot_or_tip(&self) -> bool {
        self.open.as_ref().is_some_and(|open| {
            let visible = open.text.replace('\u{200b}', "");
            let trimmed = visible.trim();
            !trimmed.is_empty()
                && (trimmed.contains('▶')
                    || trimmed.contains('✓')
                    || trimmed.contains('✗')
                    || trimmed.contains('🔎')
                    || !is_keepalive_item(&open.item_id))
        })
    }

    #[cfg(test)]
    pub(in crate::anthropic::stream) async fn activity_keepalive(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.emit_activity_heartbeat(blocks, stream).await
    }

    #[cfg(test)]
    async fn emit_activity_heartbeat(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.open.as_ref().is_some_and(|open| {
            !is_keepalive_item(&open.item_id) && open.text.replace('\u{200b}', "").trim().is_empty()
        }) {
            self.close(blocks, stream).await?;
        }
        // Never open thinking or emit synthetic bytes for silence.
        if self.open.is_none() {
            return Ok(());
        }
        self.elapsed_keepalive(blocks, std::time::Duration::ZERO, None, stream)
            .await
    }

    #[cfg(test)]
    /// SSE comment keepalives are emitted by `KeepaliveStream`; never append a
    /// synthetic ZWSP to a real thinking block.
    pub(in crate::anthropic::stream) async fn elapsed_keepalive(
        &self,
        _blocks: &[Value],
        _elapsed: std::time::Duration,
        _last_tool: Option<&str>,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        Ok(())
    }
}
