/// One-shot Command Code / Pi Luna-Spark workers.
/// Restored from `command_code_acp::prompt::is_command_code_model` after ACP
/// deletion, and extended for Pi `commandcode/` plus Codex Spark ids.
pub(crate) fn is_command_code_model(model: &str) -> bool {
    let compact = model
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_', '/', '.'], "");
    compact.contains("musespark")
        || compact.contains("commandcode")
        || compact.contains("codexspark")
}

pub(crate) fn is_canned_command_code_progress(text: &str) -> bool {
    let mut saw_content = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        saw_content = true;
        let line = line.trim_start_matches(['●', '▶', '✓', '✗', ' ']);
        let lower = line.to_ascii_lowercase();
        let thought_for = lower.strip_prefix("thought for ").is_some_and(|rest| {
            let compact: String = rest
                .trim()
                .trim_end_matches(['.', '…'])
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            let split = compact
                .find(|character: char| character.is_ascii_alphabetic())
                .unwrap_or(compact.len());
            let (number, unit) = compact.split_at(split);
            !number.is_empty()
                && number.bytes().filter(|byte| *byte == b'.').count() <= 1
                && number
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '.')
                && matches!(unit, "s" | "sec" | "secs" | "ms" | "second" | "seconds")
        });
        let canned = thought_for
            || line.starts_with("起動: Command Code")
            || line.starts_with("モデル要求中:")
            || line.contains("ツール結果待ち")
            || line.contains("続きの調査または回答")
            || line.contains("次: タスク実行")
            || line.contains("次: ツールまたは回答")
            || line.contains("次: トークン待ち")
            || line.contains("次: 別手段または報告")
            || line.contains("次: 中断")
            || ((line.starts_with("実行中:")
                || line.starts_with("完了:")
                || line.starts_with("失敗:"))
                && line.contains("。次:"))
            || (line.starts_with("ターン") && line.contains("開始"));
        if !canned {
            return false;
        }
    }
    saw_content
}

#[cfg(test)]
mod tests {
    use super::is_command_code_model;

    #[test]
    fn detects_command_code_pi_luna_spark_and_legacy_muse() {
        assert!(is_command_code_model("commandcode/gpt-5.6-luna"));
        assert!(is_command_code_model("COMMANDCODE/gpt-5.6-luna"));
        assert!(is_command_code_model("gpt-5.3-codex-spark"));
        assert!(is_command_code_model("  gpt-5.3-codex-spark  "));
        assert!(is_command_code_model("meta/muse-spark-1.2-contributor"));
        assert!(is_command_code_model("command-code"));
        assert!(!is_command_code_model("gpt-5.6-luna"));
        assert!(!is_command_code_model("grok-4.6"));
        assert!(!is_command_code_model("claude-opus-4-6"));
        assert!(!is_command_code_model(""));
    }
}
