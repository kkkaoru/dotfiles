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

enum NativeWebEvent<'a> {
    Started { query: &'a str },
    Completed { call_id: &'a str },
}

fn native_web_event(event: &Value) -> Option<NativeWebEvent<'_>> {
    let item = event.pointer("/params/item")?;
    (item.get("type").and_then(Value::as_str) == Some("webSearch")).then_some(())?;
    match event.get("method").and_then(Value::as_str) {
        Some("item/started") => Some(NativeWebEvent::Started {
            query: item
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .unwrap_or("search"),
        }),
        Some("item/completed") if native_web_succeeded(item) => item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(|call_id| NativeWebEvent::Completed { call_id }),
        _ => None,
    }
}

fn native_web_succeeded(item: &Value) -> bool {
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    !matches!(status, "failed" | "error" | "cancelled")
        && !item.get("error").is_some_and(|error| !error.is_null())
        && has_structured_retrieval_evidence(item)
}

fn has_structured_retrieval_evidence(item: &Value) -> bool {
    ["results", "sources", "evidence"]
        .into_iter()
        .filter_map(|field| item.get(field))
        .any(|value| match value {
            Value::Array(entries) => !entries.is_empty(),
            Value::Object(entries) => !entries.is_empty(),
            _ => false,
        })
}

fn requires_verified_web_evidence(messages: &[Value], system: &Value) -> bool {
    is_dedicated_live_web_worker(system)
        || messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|message| message.get("content"))
            .filter_map(content_text)
            .any(|text| explicitly_requests_live_web(text.as_str()))
}

fn is_dedicated_live_web_worker(system: &Value) -> bool {
    content_text(system).is_some_and(|text| {
        text.contains("claudex-haiku-search")
            || text.contains("Dedicated live-web retrieval worker")
            || text.contains("tools: WebSearch,WebFetch")
    })
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

fn explicitly_requests_live_web(text: &str) -> bool {
    let normalized = text.to_lowercase();
    [
        "websearch",
        "webfetch",
        "web search",
        "web fetch",
        "search the web",
        "search online",
        "live web",
        "live-web",
        "web検索",
        "ウェブ検索",
        "ウェブで検索",
        "webで検索",
        "webで調査",
        "ウェブで調査",
        "インターネットで調査",
    ]
    .into_iter()
    .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
include!("web_provenance_tests.rs");
