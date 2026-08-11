//! Evidence requirements and idempotent accounting for provider-native retrievals.
//!
//! A model may write convincing-looking URLs without calling a retrieval tool.
//! Keep that prose out of committed answers unless this turn recorded provider
//! evidence. This module intentionally examines only user content and the
//! dedicated retrieval-worker marker, never the bridge's general instructions.

use anyhow::Result;
use serde_json::{Value, json};

use super::super::protocol::StreamSender;
use super::SegmentBuilder;

#[path = "web_provenance_helpers.rs"]
mod helpers;
use helpers::{NativeWebEvent, native_web_event, requires_verified_web_evidence};
#[cfg(test)]
#[allow(unused_imports)]
use helpers::{explicitly_requests_live_web, is_dedicated_live_web_worker};

pub(super) const UNVERIFIED_WEB_RESPONSE: &str = "この結果は Web 取得を検証できないため、本文中の URL や事実は採用しません。WebSearch または WebFetch の成功結果を確認してから再試行してください。";

impl SegmentBuilder {
    /// Records one completed, provider-validated web retrieval per tool call.
    ///
    /// The caller must validate the provider evidence before invoking this; this
    /// method only makes retries and duplicated completion events idempotent.
    pub(super) fn mark_verified_web_evidence(&mut self, call_id: &str) -> bool {
        if self
            .verified_web_evidence_call_ids
            .iter()
            .any(|seen| seen == call_id)
        {
            return false;
        }
        self.verified_web_evidence_call_ids.push(call_id.to_owned());
        true
    }

    /// Whether the turn contains at least one provider-validated web retrieval.
    pub(super) fn has_verified_web_evidence(&self) -> bool {
        !self.verified_web_evidence_call_ids.is_empty()
    }

    /// Count provider-native retrievals with validated provenance for response
    /// metadata. IDs remain private so they cannot become model-authored URLs.
    pub(crate) fn verified_web_evidence_count(&self) -> u64 {
        self.verified_web_evidence_call_ids.len() as u64
    }

    /// Stores validated provider evidence once and accounts for a verified
    /// search without exposing the builder's internal usage state.
    pub(crate) fn record_verified_web_evidence(&mut self, call_id: &str) {
        if self.mark_verified_web_evidence(call_id) {
            self.usage.web_search_requests = self.usage.web_search_requests.saturating_add(1);
        }
    }

    pub(super) fn record_web_evidence_requirement(
        &mut self,
        current_messages: &[Value],
        system: &Value,
    ) {
        self.requires_verified_web_evidence |=
            requires_verified_web_evidence(current_messages, system);
    }

    pub(super) async fn native_web_search_event(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        match native_web_event(event) {
            Some(NativeWebEvent::Started { query }) => {
                self.stream_progress_text(&format!("\n\n🔎 WebSearch: {query}\n"), stream)
                    .await
            }
            Some(NativeWebEvent::Completed { call_id }) => {
                self.record_verified_web_evidence(call_id);
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub(super) fn gate_unverified_web_response(&mut self, stop_reason: &str) -> bool {
        if !self.requires_verified_web_evidence
            || self.has_verified_web_evidence()
            || stop_reason == "tool_use"
        {
            return false;
        }
        self.blocks = vec![json!({"type":"text","text":UNVERIFIED_WEB_RESPONSE})];
        true
    }
}

#[cfg(test)]
include!("web_provenance_tests.rs");
