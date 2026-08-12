use std::time::Instant;

use crate::anthropic::MessagesRequest;

use super::{core, SchedulerConfig, SchedulerDecision};

pub(crate) fn reassessment_due(
    inner: &super::Inner,
    key: &str,
    now: Instant,
    config: &SchedulerConfig,
) -> bool {
    inner
        .threads
        .get(key)
        .is_none_or(|state| now.duration_since(state.last_reassessed) >= config.reassess_interval)
}

pub(crate) fn apply_reassessment_actions(
    decision: &mut SchedulerDecision,
    snapshot: &core::SubagentSnapshot,
    request: &MessagesRequest,
    config: &SchedulerConfig,
    should_reassess: bool,
) {
    if !snapshot.has_any_workers()
        || !config.reevaluate_on_completion
        || !has_parallel_scope(request)
    {
        return;
    }
    if should_reassess {
        let cadence_minutes = config.reassess_interval.as_secs().div_ceil(60).max(1);
        decision.actions.push(format!(
            "Re-evaluate active set and issue follow-up instructions every {cadence_minutes}-minute tick"
        ));
    }
    if decision.completed_recently > 0 {
        decision
            .actions
            .push("Re-evaluate immediately after completion and rebalance lanes now".to_owned());
    }
}

/// Stated independent scopes, or observed lanes when the user turn is gone.
fn scope_target_cap(decision: &SchedulerDecision, request: &MessagesRequest) -> usize {
    if has_classifiable_user_turn(request) {
        independent_scope_count(request).max(1)
    } else {
        decision
            .active_workers
            .saturating_add(decision.completed_recently)
            .max(1)
    }
}

/// A completion or cadence tick is the only point at which a surviving lane is
/// replenished automatically.  Stable single-worker work is left alone so an
/// indivisible request does not turn the default floor into an over-spawn.
pub(crate) fn apply_replenishment_target(
    decision: &mut SchedulerDecision,
    snapshot: &core::SubagentSnapshot,
    request: &MessagesRequest,
    config: &SchedulerConfig,
    should_reassess: bool,
) {
    if !snapshot.has_any_workers()
        || !has_parallel_scope(request)
        || (decision.completed_recently == 0 && !should_reassess)
    {
        return;
    }
    let cap = scope_target_cap(decision, request).min(config.max_parallel_workers);
    if cap < 2 {
        decision.target_workers = cap;
        return;
    }
    decision.target_workers = decision
        .target_workers
        .max(config.active_floor.min(cap))
        .min(cap);
}

pub(crate) fn apply_capacity_actions(
    decision: &mut SchedulerDecision,
    target_workers: usize,
    config: &SchedulerConfig,
) {
    let (_, upper_bound) = worker_bounds(config);
    let effective_target = target_workers.min(upper_bound);
    decision.target_workers = effective_target;
    if decision.active_workers >= effective_target {
        return;
    }
    let target_gap = effective_target - decision.active_workers;
    decision.needs_more_workers = target_gap;
    decision.actions.push(format!(
        "Launch at least {target_gap} additional SubAgent lanes now (minimum floor is {}).",
        effective_target
    ));
}

fn worker_bounds(config: &SchedulerConfig) -> (usize, usize) {
    if config.min_parallel_workers <= config.max_parallel_workers {
        (config.min_parallel_workers, config.max_parallel_workers)
    } else {
        (config.max_parallel_workers, config.max_parallel_workers)
    }
}

pub(crate) fn apply_floor_action(
    decision: &mut SchedulerDecision,
    request: &MessagesRequest,
    config: &SchedulerConfig,
) {
    let rebalance_due = decision.completed_recently > 0
        || decision
            .actions
            .iter()
            .any(|action| action.contains("Re-evaluate active set"));
    if !has_parallel_scope(request)
        || !decision.has_work()
        || decision.active_workers >= config.active_floor
        || !rebalance_due
        || scope_target_cap(decision, request) < 2
    {
        return;
    }
    decision.active_floor_breached = true;
    let action = match decision.active_workers {
        1 => {
            "Only one active lane remains; interrupt stale work, dispatch replacements for unfinished branches, then continue."
        }
        _ => {
            "Active worker count is close to the operational floor; reallocate heavy work and keep replacements ready."
        }
    };
    decision.actions.push(action.to_owned());
}

pub(crate) use super::scope_count::independent_scope_count;
use super::scope_count::{has_classifiable_user_turn, has_parallel_scope};

#[path = "policy_followup.rs"]
mod followup;
pub(crate) use followup::{
    apply_diversity_action, apply_reuse_actions, clear_empty_decision, estimate_target_workers,
    persist_thread, scope_guidance,
};
