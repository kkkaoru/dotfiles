use agent_client_protocol as acp;

use super::super::{
    coalesce::remaining_final_message,
    events::{TurnResult, result_is_error, result_message, turn_cancelled_updates},
};
use super::HeadlessAgent;

pub(super) async fn emit_cancelled(
    agent: &HeadlessAgent,
    session_id: acp::SessionId,
) -> acp::Result<acp::PromptResponse> {
    for update in turn_cancelled_updates() {
        agent.notify(session_id.clone(), update).await?;
    }
    Ok(acp::PromptResponse::new(acp::StopReason::Cancelled))
}

pub(super) async fn emit_result(
    agent: &HeadlessAgent,
    session_id: acp::SessionId,
    result: &TurnResult,
    streamed_message: &str,
) -> acp::Result<acp::PromptResponse> {
    let failed = result_is_error(result);
    // Only `finalText` may become an assistant chunk. Error payloads from
    // `result_message` must not masquerade as a successful EndTurn answer.
    if let Some(text) = remaining_final_message(&result.final_text, streamed_message) {
        agent
            .notify(
                session_id,
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(text)),
                )),
            )
            .await?;
    } else if failed && streamed_message.trim().is_empty() {
        return Err(acp::Error::internal_error().data(
            result
                .error
                .clone()
                .or_else(|| {
                    let text = result_message(result);
                    (!text.trim().is_empty()).then_some(text)
                })
                .unwrap_or_else(|| "Command Code headless failed".to_owned()),
        ));
    }
    let stop =
        if result.subtype == "max_turns" || result.stop_reason.as_deref() == Some("max_turns") {
            acp::StopReason::MaxTokens
        } else {
            acp::StopReason::EndTurn
        };
    Ok(acp::PromptResponse::new(stop))
}
