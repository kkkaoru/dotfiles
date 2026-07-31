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
}
