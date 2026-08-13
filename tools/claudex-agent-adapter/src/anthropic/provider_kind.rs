pub(crate) const OPENCODE_GPT_LUNA: &str = "opencode-go/gpt-5.6-luna";
const GPT_LUNA: &str = "gpt-5.6-luna";

pub(crate) fn is_cline_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("cline/") || model.starts_with("cline-pass/")
}

pub(crate) fn is_gpt_luna_model(model: &str) -> bool {
    let model = model.trim();
    model == GPT_LUNA || model.ends_with("/gpt-5.6-luna")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cline_credits_and_clinepass_models() {
        assert!(is_cline_model("cline-pass/deepseek-v4-flash"));
        assert!(is_cline_model("cline/deepseek-v4-flash"));
        assert!(!is_cline_model("gpt-5.6-luna"));
        assert!(!is_cline_model("opencode-go/gpt-5.6-luna"));
        assert!(!is_cline_model("qwen3.8-max-preview"));
    }

    #[test]
    fn detects_codex_and_opencode_gpt_luna() {
        assert!(is_gpt_luna_model("gpt-5.6-luna"));
        assert!(is_gpt_luna_model(OPENCODE_GPT_LUNA));
        assert!(!is_gpt_luna_model("gpt-5.3-codex-spark"));
        assert!(!is_gpt_luna_model("cline-pass/deepseek-v4-flash"));
    }
}
