use anyhow::{Context, Result};
use serde_json::json;

use super::super::SegmentBuilder;
use super::super::external_tool::ExternalToolContext;
use crate::anthropic::stream::ToolCall;
use crate::anthropic::token_efficiency::{
    UnusableCircuitDecision, consecutive_unusable_from_messages, decide_tool_emit,
    tool_arguments_are_unusable,
};

const UNUSABLE_TOOL_REJECT: &str =
    "Adapter rejected unusable tool arguments. Supply complete JSON with the required keys.";
const CIRCUIT_OPEN_REJECT: &str = "Stopped emitting tool_use after 3 consecutive unusable tool arguments or InputValidationError.";

impl SegmentBuilder {
    pub(super) async fn skip_unusable_or_tripped_tool(
        &mut self,
        context: ExternalToolContext<'_>,
        call: &ToolCall,
    ) -> Result<bool> {
        let from_messages = consecutive_unusable_from_messages(context.current_messages);
        let unusable = tool_arguments_are_unusable(&call.name, &call.arguments);
        match decide_tool_emit(self.consecutive_invalid_tool_json, from_messages, unusable) {
            UnusableCircuitDecision::Emit { consecutive } => {
                self.consecutive_invalid_tool_json = consecutive;
                Ok(false)
            }
            UnusableCircuitDecision::Skip {
                consecutive,
                end_turn,
            } => {
                self.consecutive_invalid_tool_json = consecutive;
                self.suppressed_tool_use |= end_turn;
                reject_skipped_tool(context, call, reject_message(end_turn)).await?;
                Ok(true)
            }
        }
    }
}

fn reject_message(end_turn: bool) -> &'static str {
    if end_turn {
        CIRCUIT_OPEN_REJECT
    } else {
        UNUSABLE_TOOL_REJECT
    }
}

async fn reject_skipped_tool(
    context: ExternalToolContext<'_>,
    call: &ToolCall,
    message: &str,
) -> Result<()> {
    context
        .bridge
        .app_for_session(context.session)
        .respond_for_model(
            &context.session.model,
            call.request_id.clone(),
            json!({
                "contentItems":[{"type":"inputText","text":message}],
                "success":false
            }),
        )
        .await
        .context("failed to reject an unusable provider tool")
}
