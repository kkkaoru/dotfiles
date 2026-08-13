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
    let t = text.trim().to_ascii_lowercase();
    let Some(rest) = t.strip_prefix("thought for ") else {
        return false;
    };
    is_elapsed_duration(rest)
}

fn is_elapsed_duration(rest: &str) -> bool {
    let compact: String = rest
        .trim()
        .trim_end_matches(['.', '…'])
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if compact.is_empty() {
        return true;
    }
    let (num, unit) = match compact.find(|c: char| c.is_ascii_alphabetic()) {
        Some(idx) => compact.split_at(idx),
        None => (compact.as_str(), ""),
    };
    if num.is_empty()
        || num.bytes().filter(|b| *b == b'.').count() > 1
        || !num.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return false;
    }
    matches!(unit, "s" | "sec" | "secs" | "ms" | "second" | "seconds")
}

/// Hold char-by-char Muse deltas until `Thought for 15s` / launch chrome is complete.
pub(crate) fn is_incomplete_canned_prefix(text: &str) -> bool {
    if text.contains('\n') {
        return false;
    }
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    if "thought for ".starts_with(&lower) {
        return true;
    }
    if lower.starts_with("thought for ") && !is_thought_for_chrome(t) {
        let rest = lower["thought for ".len()..].trim();
        return rest.is_empty()
            || rest
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c.is_whitespace())
            || is_partial_time_unit(rest);
    }
    ["起動: Command Code", "モデル要求中:"]
        .iter()
        .any(|prefix| prefix.starts_with(t) && *prefix != t)
}

fn is_partial_time_unit(rest: &str) -> bool {
    let Some(idx) = rest.find(|c: char| c.is_ascii_alphabetic()) else {
        return false;
    };
    let (num, unit) = rest.split_at(idx);
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    ["s", "sec", "secs", "ms", "second", "seconds"]
        .iter()
        .any(|full| full.starts_with(unit) && *full != unit)
}

fn is_canned_line(text: &str) -> bool {
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

/// Drop canned Command Code chrome lines and keep any real remainder.
pub fn strip_canned_progress(text: &str) -> Option<String> {
    let mut kept = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if body.trim().is_empty() || is_canned_line(body) {
            continue;
        }
        if !kept.is_empty() {
            kept.push('\n');
        }
        kept.push_str(body.trim());
    }
    if kept.is_empty() {
        return None;
    }
    if text.ends_with('\n') && !kept.ends_with('\n') {
        kept.push('\n');
    }
    Some(kept)
}

/// True only when every non-empty line is canned chrome — not mixed CoT.
pub fn is_canned_progress(text: &str) -> bool {
    !text.trim().is_empty() && strip_canned_progress(text).is_none()
}

pub(super) fn has_status_prefix(text: &str) -> bool {
    text.starts_with('●') || text.starts_with('▶') || text.starts_with('✓') || text.starts_with('✗')
}

fn message(text: impl Into<String>) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text.into()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_predicates_cover_all_status_and_duration_forms() {
        for text in [
            "Thought for .",
            "Thought for 1s",
            "Thought for 1sec",
            "Thought for 1secs",
            "Thought for 1ms",
            "Thought for 1second",
            "Thought for 1seconds",
        ] {
            assert!(is_thought_for_chrome(text));
        }
        for text in ["Thought for 1.2.3s", "Thought for xs", "Thought for 1x"] {
            assert!(!is_thought_for_chrome(text));
        }
        for rest in ["", "1.2s", "1ms", "1second"] {
            assert!(is_elapsed_duration(rest));
        }
        for rest in ["1.2.3s", "xs", "1x"] {
            assert!(!is_elapsed_duration(rest));
        }
        for text in [
            "ツール結果待ち",
            "続きの調査または回答",
            "次: タスク実行",
            "次: ツールまたは回答",
            "次: トークン待ち",
            "次: 別手段または報告",
            "次: 中断",
            "起動: Command Code",
            "実行中: x。次: y",
            "完了: x。次: y",
            "失敗: x。次: y",
            "ターン1開始",
            "モデル要求中: x",
        ] {
            assert!(is_canned_line(text), "{text}");
        }
        assert!(!is_canned_line("ordinary text"));
        for marker in ['●', '▶', '✓', '✗'] {
            assert!(is_canned_line(&format!("{marker} 次: 中断")));
        }
        for text in [
            "次: 別のタスク",
            "実行中: x",
            "完了: x",
            "失敗: x",
            "ターン1終了",
        ] {
            assert!(!is_canned_line(text), "{text}");
        }
        assert!(!is_partial_time_unit("1seconds"));
        assert!(is_partial_time_unit("1se"));
        for marker in ['●', '▶', '✓', '✗'] {
            assert!(has_status_prefix(&format!("{marker} status")));
        }
        assert!(!has_status_prefix("status"));
    }
}
