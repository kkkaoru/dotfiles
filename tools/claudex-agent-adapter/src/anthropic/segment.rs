use serde_json::{Value, json};

#[derive(Debug)]
pub(super) struct Segment {
    pub(super) blocks: Vec<Value>,
    pub(super) stop_reason: &'static str,
    pub(super) usage: Usage,
    pub(super) web_evidence: WebEvidenceSummary,
    /// Next Anthropic SSE content index after blocks already sent this turn.
    /// Sanitizing empty thinking does not free those wire indices.
    pub(super) next_sse_index: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Usage {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_creation_1h_input_tokens: Option<u64>,
    pub(super) web_search_requests: u64,
}

impl Usage {
    pub(super) fn apply_anthropic_details(self, usage: &mut Value) {
        usage["cache_read_input_tokens"] = json!(self.cache_read_input_tokens);
        usage["cache_creation_input_tokens"] = json!(self.cache_creation_input_tokens);
        usage["output_tokens_details"] = json!({"thinking_tokens":self.reasoning_output_tokens});
        if let Some(one_hour) = self.cache_creation_1h_input_tokens {
            let one_hour = one_hour.min(self.cache_creation_input_tokens);
            usage["cache_creation"] = json!({
                "ephemeral_1h_input_tokens":one_hour,
                "ephemeral_5m_input_tokens":self.cache_creation_input_tokens - one_hour
            });
        }
    }
}

/// Immutable, aggregate-only evidence record carried from the stream builder
/// to the Anthropic response serializers. It deliberately excludes provider
/// URLs, result text, and model prose.
#[derive(Clone, Copy, Debug, Default)]
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

    #[cfg(test)]
    pub(super) fn with_empty_end_turn_notice(mut self) -> Self {
        if !self.is_empty_end_turn() {
            return self;
        }
        self.blocks
            .push(json!({"type":"text","text": EMPTY_ASSISTANT_TURN}));
        self
    }
}

fn block_has_assistant_payload(block: &Value) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => visible_assistant_text(block.get("text").and_then(Value::as_str)),
        Some("thinking") => visible_assistant_text(block.get("thinking").and_then(Value::as_str)),
        Some("tool_use") | Some("server_tool_use") => true,
        Some(_) => true,
        None => false,
    }
}

fn visible_assistant_text(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.replace('\u{200b}', "").trim().is_empty())
}

/// Shared wording for ACP providers that swallow billing/auth failures as empty
/// `end_turn` responses (observed with Cline when Credits balance is $0).
pub(super) const EMPTY_ACP_END_TURN: &str = "Configured ACP completed with no assistant content \
(provider likely unavailable or billing exhausted; Cline Credits models return empty end_turn \
when balance is $0 — use Qwen Cloud `qwen3.8-max-preview` / `claudex-qwen`, or top up Credits)";

pub(super) const CONTEXT_WINDOW_AFTER_MESSAGE_START: &str = "Context window overflow after \
message_start; a retry was unavailable. No assistant content was produced. Compact or start a \
new turn; this is not successful work.";

pub(super) const NO_ASSISTANT_SUBSTITUTE: &str = "No assistant content was produced after the \
stream started (no provider output or unusable tools). This is not a completed answer.";

pub(super) const UNUSABLE_TOOLS_SUBSTITUTE: &str = "No assistant content was produced: tool \
arguments were unusable after 3 consecutive failures, so tool_use was suppressed. This is not \
successful work.";

pub(super) fn primed_empty_turn_notice(error: &str) -> String {
    let body = primed_empty_turn_notice_body(error);
    let detail = error.trim();
    if detail.is_empty() || detail == body || body.contains(detail) {
        return body.to_owned();
    }
    format!("{body} ({detail})")
}

fn primed_empty_turn_notice_body(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if is_context_window_notice(error) {
        CONTEXT_WINDOW_AFTER_MESSAGE_START
    } else if contains_empty_acp_billing_marker(error) {
        EMPTY_ACP_END_TURN
    } else if contains_empty_assistant_turn_marker(error) {
        EMPTY_ASSISTANT_TURN
    } else if lower.contains("stopped emitting tool_use after") {
        UNUSABLE_TOOLS_SUBSTITUTE
    } else {
        NO_ASSISTANT_SUBSTITUTE
    }
}

fn is_context_window_notice(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("context window")
        || error.contains("ran out of room")
        || error.contains("contextwindowexceeded")
        || error.contains("context_window_exceeded")
        || error.contains("context limit")
}

/// Generic Pi/Claude empty `end_turn`. Distinct from Cline billing so Luna /
/// Command Code empty completions do not record an empty-acp-billing cooldown.
pub(super) const EMPTY_ASSISTANT_TURN: &str = "Provider completed with no assistant content. \
The route returned no assistant text or tools. This is a failure, not a completed result.";

pub(super) const EMPTY_ASSISTANT_RETRY_MARKER: &str = "claudex-empty-assistant-retry";

pub(super) const EMPTY_ASSISTANT_RETRY_PROMPT: &str = "Claudex (claudex-empty-assistant-retry): \
previous provider turn completed with no assistant content. That empty turn is a failure. \
Continue with tools or visible assistant text. Do not emit another empty end_turn.";

/// Detect empty-ACP billing/auth failures so SubAgent turns can cool down the
/// exhausted provider and failover to a sibling (for example Qwen Cloud).
pub(crate) fn contains_empty_acp_billing_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("configured acp completed with no assistant content")
        || (value.contains("cline credits") && value.contains("empty end_turn"))
}

pub(crate) fn contains_empty_assistant_turn_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("provider completed with no assistant content")
        || value.contains("context window overflow after")
        || value.contains("no assistant content was produced")
        || value.contains(EMPTY_ASSISTANT_RETRY_MARKER)
}

pub(crate) fn messages_already_retried_empty_assistant(messages: &[Value]) -> bool {
    messages
        .iter()
        .any(|message| message.to_string().contains(EMPTY_ASSISTANT_RETRY_MARKER))
}

pub(crate) fn empty_assistant_retry_user_message() -> Value {
    json!({
        "role": "user",
        "content": [{
            "type": "text",
            "text": EMPTY_ASSISTANT_RETRY_PROMPT
        }]
    })
}

/// ACP `-32603` from a real Cline launch with Credits $0. Terminal and labeled
/// Cline; do not wrap as a Codex app-server 502 or hang on retries.
pub(crate) const CLINE_CREDITS_FAILURE: &str = "Cline ACP prompt failed: Cline Credits \
insufficient balance. This is a Cline billing failure, not a Codex error. Top up Credits \
or launch a non-Cline worker. Do not retry.";

pub(crate) fn contains_cline_credits_balance_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("insufficient balance")
        || value.contains("app.cline.bot/credits")
        || (value.contains("cline credits") && (value.contains("$0") || value.contains("0.00")))
}

pub(crate) fn cline_credits_failure_message(detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        return CLINE_CREDITS_FAILURE.to_owned();
    }
    format!("{CLINE_CREDITS_FAILURE} ({detail})")
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
            next_sse_index: 0,
        };
        assert!(empty.is_empty_end_turn());
        let noticed = empty.with_empty_end_turn_notice();
        assert!(!noticed.is_empty_end_turn());
        assert_eq!(noticed.blocks.len(), 1);
        assert_eq!(
            noticed.blocks[0]["text"],
            "Provider completed with no assistant content. The route returned no assistant text or tools. This is a failure, not a completed result."
        );
    }

    #[test]
    fn primed_empty_turn_notice_uses_context_window_and_no_assistant_literals() {
        assert_eq!(
            primed_empty_turn_notice("context window exceeded: input_tokens 138765 > limit 110000"),
            "Context window overflow after message_start; a retry was unavailable. No assistant content was produced. Compact or start a new turn; this is not successful work. (context window exceeded: input_tokens 138765 > limit 110000)"
        );
        assert_eq!(
            primed_empty_turn_notice("provider emitted nothing"),
            "No assistant content was produced after the stream started (no provider output or unusable tools). This is not a completed answer. (provider emitted nothing)"
        );
        assert_eq!(
            primed_empty_turn_notice(EMPTY_ACP_END_TURN),
            EMPTY_ACP_END_TURN
        );
        assert_eq!(primed_empty_turn_notice("   "), NO_ASSISTANT_SUBSTITUTE);
        assert!(contains_empty_assistant_turn_marker(
            CONTEXT_WINDOW_AFTER_MESSAGE_START
        ));
    }

    #[test]
    fn treats_zwsp_only_thinking_as_empty_end_turn() {
        let empty = Segment {
            blocks: vec![json!({"type":"thinking","thinking":"\u{200b}"})],
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
            next_sse_index: 0,
        };
        assert!(empty.is_empty_end_turn());
    }

    #[test]
    fn generic_empty_assistant_is_not_cline_billing() {
        assert!(contains_empty_assistant_turn_marker(EMPTY_ASSISTANT_TURN));
        assert!(!contains_empty_acp_billing_marker(EMPTY_ASSISTANT_TURN));
        assert!(!contains_empty_assistant_turn_marker(EMPTY_ACP_END_TURN));
        assert!(!messages_already_retried_empty_assistant(&[
            json!({"role":"user","content":"retry me"})
        ]));
        assert!(messages_already_retried_empty_assistant(&[
            empty_assistant_retry_user_message()
        ]));
        assert_eq!(
            primed_empty_turn_notice(EMPTY_ASSISTANT_TURN),
            EMPTY_ASSISTANT_TURN
        );
        assert!(
            primed_empty_turn_notice(
                "Stopped emitting tool_use after 3 consecutive empty or invalid JSON payloads."
            )
            .contains("tool arguments were unusable")
        );
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
        assert!(contains_empty_acp_billing_marker(
            "Cline Credits models return empty end_turn when balance is $0"
        ));
    }

    #[test]
    fn detects_cline_credits_insufficient_balance() {
        assert!(contains_cline_credits_balance_marker(
            "Internal error: Insufficient balance. Add credits at https://app.cline.bot/credits"
        ));
        assert!(contains_cline_credits_balance_marker("Cline Credits $0.00"));
        assert!(!contains_cline_credits_balance_marker(
            "codex app-server turn failed: connection reset"
        ));
        assert!(cline_credits_failure_message("Insufficient balance").contains("Cline Credits"));
        assert!(
            !cline_credits_failure_message("Insufficient balance").contains("codex app-server")
        );
        assert!(contains_cline_credits_balance_marker(
            "Top up at https://app.cline.bot/credits before retrying"
        ));
        assert!(!contains_cline_credits_balance_marker(
            "Cline Credits depleted without a listed amount"
        ));
        assert_eq!(cline_credits_failure_message("   "), CLINE_CREDITS_FAILURE);
    }

    #[test]
    fn keeps_text_thinking_and_tool_turns() {
        let text = Segment {
            blocks: vec![json!({"type":"text","text":"PONG"})],
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
            next_sse_index: 0,
        };
        assert!(!text.is_empty_end_turn());
        let thinking = Segment {
            blocks: vec![json!({"type":"thinking","thinking":"plan"})],
            stop_reason: "end_turn",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
            next_sse_index: 0,
        };
        assert!(!thinking.is_empty_end_turn());
        let tools = Segment {
            blocks: vec![json!({"type":"tool_use","id":"1","name":"Bash","input":{}})],
            stop_reason: "tool_use",
            usage: Usage::default(),
            web_evidence: WebEvidenceSummary::default(),
            next_sse_index: 0,
        };
        assert!(!tools.is_empty_end_turn());
    }
}
