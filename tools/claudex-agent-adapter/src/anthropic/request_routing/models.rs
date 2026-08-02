/// Official Claude API alias for Claude Haiku 4.5.
///
/// Source: <https://platform.claude.com/docs/ja/about-claude/models/overview>
pub(super) const CLAUDE_HAIKU_MODEL: &str = "claude-haiku-4-5";
pub(super) const CLAUDE_LONG_CONTEXT_MODEL: &str = concat!("claude-", "opus", "-", "5", "[1m]");

pub(crate) fn official_claude_haiku_model() -> &'static str {
    CLAUDE_HAIKU_MODEL
}

pub(super) fn normalize_claude_model_to_haiku(model: &str) -> Option<&'static str> {
    is_native_claude_model(model).then_some(CLAUDE_HAIKU_MODEL)
}

fn is_native_claude_model(model: &str) -> bool {
    matches!(model, "fable" | "opus" | "sonnet" | "haiku")
        || model.starts_with("claude-")
        || model.starts_with("fable[")
        || model.starts_with("opus[")
        || model.starts_with("sonnet[")
        || model.starts_with("haiku[")
}

#[cfg(test)]
mod tests {
    use super::{CLAUDE_HAIKU_MODEL, normalize_claude_model_to_haiku};

    #[test]
    fn normalizes_unowned_native_claude_models_to_the_haiku_alias() {
        for model in [
            "claude-sonnet-5",
            "sonnet",
            "haiku",
            "fable[1m]",
            "opus[1m]",
            "sonnet[1m]",
            "haiku[1m]",
        ] {
            assert_eq!(
                normalize_claude_model_to_haiku(model),
                Some(CLAUDE_HAIKU_MODEL)
            );
        }
        assert_eq!(normalize_claude_model_to_haiku("gpt-5"), None);
    }
}
