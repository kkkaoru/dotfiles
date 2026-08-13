use crate::anthropic::MessagesRequest;

#[path = "scope_count/actions.rs"]
mod actions;
#[path = "scope_count/content.rs"]
mod content;
#[path = "scope_count_detect.rs"]
mod detect;
#[path = "scope_count/filters.rs"]
mod filters;

use actions::{count_for_content, declines_delegation_text, is_atomic_lookup};
use content::last_real_user_text;
use detect::{contains_parallel_intent, contains_substantive_verb};
use filters::remove_negative_or_diagnostic_lines;

pub(super) const MAX_STATED_SCOPES: usize = 40;
pub(super) const CARDINALITY_WINDOW: usize = 24;

pub(crate) fn has_parallel_scope(request: &MessagesRequest) -> bool {
    independent_scope_count(request) >= 2
}

pub(crate) fn independent_scope_count(request: &MessagesRequest) -> usize {
    match last_real_user_text(request) {
        Some(content) => count_for_content(&content),
        // Reconstructed transcripts keep a parallel baseline for floor/replenishment.
        None => 2,
    }
}

pub(crate) fn has_classifiable_user_turn(request: &MessagesRequest) -> bool {
    last_real_user_text(request).is_some()
}

pub(crate) fn declines_delegation(request: &MessagesRequest) -> bool {
    last_real_user_text(request).is_some_and(|content| declines_delegation_text(&content))
}

pub(crate) fn needs_single_worker(request: &MessagesRequest) -> bool {
    if declines_delegation(request) || independent_scope_count(request) >= 2 {
        return false;
    }
    last_real_user_text(request).is_some_and(|content| is_atomic_lookup(&content))
}

pub(crate) fn is_substantive_work(request: &MessagesRequest) -> bool {
    let Some(content) = last_real_user_text(request) else {
        return false;
    };
    if declines_delegation_text(&content) {
        return false;
    }
    let semantic = remove_negative_or_diagnostic_lines(&content);
    count_for_content(&semantic) >= 2
        || contains_parallel_intent(&semantic)
        || contains_substantive_verb(&semantic)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "scope_count/action_edge_tests.rs"]
mod action_edge_tests;
#[cfg(test)]
#[path = "scope_count/behavior_tests.rs"]
mod behavior_tests;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "scope_count/detect_edge_tests.rs"]
mod detect_edge_tests;
#[cfg(test)]
#[path = "scope_count/filters_tests.rs"]
mod filters_tests;
#[cfg(test)]
#[path = "scope_count/provenance_tests.rs"]
mod provenance_tests;
#[cfg(test)]
#[path = "scope_count/test_support.rs"]
mod test_support;
