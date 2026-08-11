use agent_client_protocol as acp;

pub(super) fn ensure_trailing_newline(text: &str) -> String {
    format!("{text}\n")
}

pub(super) fn native_message(text: &str) -> acp::SessionUpdate {
    message(ensure_trailing_newline(text.trim()))
}

pub(super) fn thought_chunk(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(ensure_trailing_newline(text.trim())),
    )))
}

fn is_thought_for_chrome(text: &str) -> bool {
    text.trim().to_ascii_lowercase().starts_with("thought for ")
}

pub(super) fn is_canned_progress(text: &str) -> bool {
    let t = text.trim().trim_start_matches(['●', '▶', '✓', '✗', ' ']);
    is_thought_for_chrome(t)
        || t.contains("ツール結果待ち")
        || t.contains("続きの調査または回答")
        || t.contains("次: タスク実行")
        || t.contains("次: ツールまたは回答")
        || t.contains("次: トークン待ち")
        || t.contains("次: 別手段または報告")
        || t.contains("次: 中断")
        || t.starts_with("起動: Command Code")
        || (t.starts_with("実行中:") && t.contains("。次:"))
        || (t.starts_with("完了:") && t.contains("。次:"))
        || (t.starts_with("失敗:") && t.contains("。次:"))
        || (t.starts_with("ターン") && t.contains("開始"))
        || t.starts_with("モデル要求中:")
}

pub(super) fn has_status_prefix(text: &str) -> bool {
    text.starts_with('●') || text.starts_with('▶') || text.starts_with('✓') || text.starts_with('✗')
}

fn message(text: impl Into<String>) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text.into()),
    )))
}
