use serde_json::{Value, json};

pub(super) struct Segment {
    pub(super) blocks: Vec<Value>,
    pub(super) stop_reason: &'static str,
    pub(super) usage: Usage,
    pub(super) web_evidence: WebEvidenceSummary,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Usage {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) web_search_requests: u64,
}

/// Immutable, aggregate-only evidence record carried from the stream builder
/// to the Anthropic response serializers. It deliberately excludes provider
/// URLs, result text, and model prose.
#[derive(Clone, Copy, Default)]
pub(crate) struct WebEvidenceSummary {
    verified_count: u64,
}

impl WebEvidenceSummary {
    pub(crate) const fn from_verified_count(verified_count: u64) -> Self {
        Self { verified_count }
    }

    pub(crate) fn metadata(self) -> Option<Value> {
        (self.verified_count > 0).then(|| {
            json!({
                "claudex": {
                    "web_evidence": {
                        "evidence_class_counts": {"verified_retrieval": self.verified_count},
                        "verified_count": self.verified_count
                    }
                }
            })
        })
    }
}

impl Segment {
    pub(crate) fn with_web_evidence(mut self, web_evidence: WebEvidenceSummary) -> Self {
        self.web_evidence = web_evidence;
        self
    }

    /// Cline (and some other ACP providers) can finish with `end_turn` and no
    /// text/thinking/tools when billing or auth fails (for example Cline Credits
    /// balance $0). Treat that as an empty completed turn rather than a valid
    /// assistant reply so Claude Code does not see "No assistant messages found".
    pub(super) fn is_empty_end_turn(&self) -> bool {
        self.stop_reason == "end_turn" && !self.blocks.iter().any(block_has_assistant_payload)
    }
}

fn block_has_assistant_payload(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty()),
        Some("thinking") => block
            .get("thinking")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty()),
        Some("tool_use") | Some("server_tool_use") => true,
        Some(_) => true,
        None => false,
    }
}

/// Shared wording for ACP providers that swallow billing/auth failures as empty
/// `end_turn` responses (observed with Cline when Credits balance is $0).
pub(super) const EMPTY_ACP_END_TURN: &str = "Configured ACP completed with no assistant content \
(provider likely unavailable or billing exhausted; Cline Credits models return empty end_turn \
when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / `claudex-qwen`, or top up Credits)";

/// Detect empty-ACP billing/auth failures so SubAgent turns can cool down the
/// exhausted provider and failover to a sibling (for example Qwen Cloud).
pub(crate) fn contains_empty_acp_billing_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("configured acp completed with no assistant content")
        || (value.contains("cline credits") && value.contains("empty end_turn"))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_empty_end_turn_without_blocks() {
        let empty = Segment {
            blocks: Vec::new(),
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
        };
        assert!(empty.is_empty_end_turn());
    }

    #[test]
    fn detects_empty_acp_billing_marker_from_shared_wording() {
        assert!(contains_empty_acp_billing_marker(EMPTY_ACP_END_TURN));
        assert!(contains_empty_acp_billing_marker(
            "Agent \"Verify r2-catalog inherited edits\" failed: Agent terminated early due to an API error: API Error: Configured ACP completed with no assistant content (provider likely unavailable or billing exhausted; Cline Credits models return empty end_turn when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / `claudex-qwen`, or top up Credits)"
        ));
        assert!(contains_empty_acp_billing_marker(
            "Agent \"Verify weight-triggered re-prediction\" failed: Agent terminated early due to an API error: API Error: 502 Configured ACP completed with no assistant content (provider likely unavailable or billing exhausted; Cline Credits models return empty end_turn when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / `claudex-qwen`, or top up Credits)"
        ));
        assert!(!contains_empty_acp_billing_marker("usage limit exceeded"));
        assert!(!contains_empty_acp_billing_marker(
            "Unable to connect to API (ConnectionRefused)"
        ));
    }

    #[test]
    fn keeps_text_thinking_and_tool_turns() {
        let text = Segment {
            blocks: vec![json!({"type":"text","text":"PONG"})],
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
        };
        assert!(!text.is_empty_end_turn());
        let thinking = Segment {
            blocks: vec![json!({"type":"thinking","thinking":"plan"})],
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
        };
        assert!(!thinking.is_empty_end_turn());
        let tools = Segment {
            blocks: vec![json!({"type":"tool_use","id":"1","name":"Bash","input":{}})],
            stop_reason: "tool_use",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
        };
        assert!(!tools.is_empty_end_turn());
    }
}
