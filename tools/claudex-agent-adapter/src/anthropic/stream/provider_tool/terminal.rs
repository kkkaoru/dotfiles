use anyhow::Result;
use serde_json::Value;

use super::{
    SegmentBuilder,
    preview::{
        FAILED_STATUS_PREVIEW_CHAR_LIMIT, failure_preview, progress_start_line,
        terminal_already_emitted, truncate_for_status, validated_provider_web_evidence,
    },
};
use crate::anthropic::stream::protocol::StreamSender;

impl SegmentBuilder {
    pub(in crate::anthropic::stream) async fn finish_provider_tool_terminal(
        &mut self,
        call_id: Option<&str>,
        short_title: &str,
        params: &Value,
        success: bool,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if success {
            return self
                .emit_terminal_success(call_id, short_title, stream)
                .await;
        }
        self.emit_terminal_failure(call_id, short_title, params, stream)
            .await
    }

    async fn emit_terminal_failure(
        &mut self,
        call_id: Option<&str>,
        short_title: &str,
        params: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if terminal_already_emitted(&mut self.provider_tool_terminal_ids, call_id) {
            return Ok(());
        }
        let detail = failure_preview(params.get("output"));
        let preview = truncate_for_status(&detail, FAILED_STATUS_PREVIEW_CHAR_LIMIT);
        self.stream_progress_text(&format!("\n✗ {short_title}: {preview}\n"), stream)
            .await
    }

    async fn emit_terminal_success(
        &mut self,
        call_id: Option<&str>,
        short_title: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if terminal_already_emitted(&mut self.provider_tool_terminal_ids, call_id) {
            return Ok(());
        }
        self.stream_progress_text(&format!("\n✓ {short_title}\n"), stream)
            .await
    }

    pub(in crate::anthropic::stream) async fn start_provider_tool_from_update(
        &mut self,
        call_id: Option<&str>,
        title: &str,
        params: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(call_id) = call_id else {
            return Ok(());
        };
        if !self.remember_provider_tool(call_id, title) {
            return Ok(());
        }
        self.stream_progress_text(&progress_start_line(title, params.get("arguments")), stream)
            .await
    }

    pub(super) fn remember_provider_tool(&mut self, call_id: &str, title: &str) -> bool {
        if self.provider_tool_title(call_id).is_some() {
            return false;
        }
        self.provider_tool_calls
            .push((call_id.to_owned(), title.to_owned()));
        true
    }

    pub(super) fn provider_tool_title(&self, call_id: &str) -> Option<&str> {
        self.provider_tool_calls
            .iter()
            .find(|(seen, _)| seen == call_id)
            .map(|(_, title)| title.as_str())
    }

    pub(in crate::anthropic::stream) fn record_provider_web_evidence(
        &mut self,
        call_id: &str,
        params: &Value,
    ) {
        if !validated_provider_web_evidence(params.get("evidence")) {
            return;
        }
        self.record_verified_web_evidence(call_id);
    }
}
