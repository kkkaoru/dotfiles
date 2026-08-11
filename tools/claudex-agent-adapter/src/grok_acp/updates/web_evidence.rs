//! Correlate explicit provider-native web tools with their completion updates.
//!
//! ACP tool categories are intentionally broad: `Search` can mean repository
//! search. This tracker stores only an explicit WebSearch/WebFetch candidate and
//! emits it once the provider reports that exact call as completed.

use std::collections::HashMap;
use std::sync::Mutex;

#[path = "web_evidence_format.rs"]
mod format;
pub(super) use format::{completion_evidence, web_operation};
#[cfg(test)]
use agent_client_protocol::{self as acp};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use format::{extract_source_urls, is_http_url, meaningful_provider_output, provider_output, summary};

pub(super) const MAX_TRACKED_CALLS: usize = 256;
pub(super) const MAX_RESULT_SUMMARY_CHARS: usize = 320;
pub(super) const MAX_SOURCE_URLS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WebOperation {
    pub(super) kind: &'static str,
    pub(super) query: Option<String>,
    pub(super) url: Option<String>,
}

#[derive(Default)]
pub(super) struct ProviderWebEvidence {
    calls: Mutex<HashMap<(String, String), TrackedOperation>>,
}

#[derive(Clone, Debug)]
struct TrackedOperation {
    operation: WebOperation,
    completed: bool,
}

impl ProviderWebEvidence {
    pub(super) fn record(&self, session_id: &str, call_id: &str, operation: WebOperation) {
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        if calls.contains_key(&key) || calls.len() >= MAX_TRACKED_CALLS {
            return;
        }
        calls.insert(
            key,
            TrackedOperation {
                operation,
                completed: false,
            },
        );
    }

    pub(super) fn completion_candidate(
        &self,
        session_id: &str,
        call_id: &str,
        direct_operation: Option<WebOperation>,
    ) -> Option<WebOperation> {
        let Ok(mut calls) = self.calls.lock() else {
            return None;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        if let Some(operation) = direct_operation
            && !calls.contains_key(&key)
            && calls.len() < MAX_TRACKED_CALLS
        {
            calls.insert(
                key.clone(),
                TrackedOperation {
                    operation,
                    completed: false,
                },
            );
        }
        let tracked = calls.get(&key)?;
        if tracked.completed {
            return None;
        }
        Some(tracked.operation.clone())
    }

    pub(super) fn mark_completed(&self, session_id: &str, call_id: &str) -> bool {
        let Ok(mut calls) = self.calls.lock() else {
            return false;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        let Some(tracked) = calls.get_mut(&key) else {
            return false;
        };
        if tracked.completed {
            return false;
        }
        tracked.completed = true;
        true
    }

    #[cfg(test)]
    pub(super) fn complete(
        &self,
        session_id: &str,
        call_id: &str,
        direct_operation: Option<WebOperation>,
    ) -> Option<WebOperation> {
        let operation = self.completion_candidate(session_id, call_id, direct_operation)?;
        self.mark_completed(session_id, call_id)
            .then_some(operation)
    }

    pub(super) fn clear(&self, session_id: &str) {
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        calls.retain(|(session, _), _| session != session_id);
    }
}


#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    include!("web_evidence_tests.rs");
}
