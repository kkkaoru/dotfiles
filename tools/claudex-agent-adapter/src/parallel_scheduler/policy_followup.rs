use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::anthropic::MessagesRequest;

use super::super::{Inner, SchedulerConfig, SchedulerDecision, core, scope_count};
use scope_count::{has_parallel_scope, independent_scope_count};

fn diversity_family_cap(decision: &SchedulerDecision, config: &SchedulerConfig) -> usize {
    let worker_cap = if decision.target_workers > 0 {
        decision.target_workers
    } else {
        decision.active_workers
    };
    config.min_model_families.min(worker_cap)
}

pub(crate) fn apply_diversity_action(
    decision: &mut SchedulerDecision,
    request: &MessagesRequest,
    config: &SchedulerConfig,
) {
    if !has_parallel_scope(request) || decision.active_workers == 0 {
        return;
    }
    let required_families = diversity_family_cap(decision, config);
    if required_families < 2 || decision.active_model_families >= required_families {
        return;
    }
    decision.needs_model_diversity = true;
    decision.actions.push(format!(
        "Diversify providers: active models should cover at least {required_families} families."
    ));
}

pub(crate) fn apply_reuse_actions(
    decision: &mut SchedulerDecision,
    request: &MessagesRequest,
    config: &SchedulerConfig,
) {
    if !has_parallel_scope(request) {
        return;
    }
    if config.allow_reuse && decision.has_work() {
        decision.actions.push(
            "Prefer reusing compatible completed workers via Agent/Task resume=<agentId>; add new tasks to the same workers when their context fits. Independent scopes still need distinct launches."
                .to_owned(),
        );
    }
    if decision.completed_recently == 0 || !decision.has_work() {
        return;
    }
    decision.actions.push(
        "After each completion, audit unfinished scopes, send completion-aware follow-ups to remaining workers, then launch replacements for the still-open branches."
            .to_owned(),
    );
    decision.actions.push(
        "If scope remains weak, replay the same high-value subtask on fresh workers and expand scope on surviving active workers."
            .to_owned(),
    );
    if config.cleanup_on_exit {
        decision.actions.push(
            "After completion, reclaim obsolete or idle lanes; keep reusable workers for unfinished independent scopes."
                .to_owned(),
        );
    }
}

pub(crate) fn clear_empty_decision(
    decision: &mut SchedulerDecision,
    snapshot: &core::SubagentSnapshot,
) {
    if snapshot.has_any_workers()
        || decision.completed_recently > 0
        || decision.target_workers > 0
        || decision.needs_more_workers > 0
    {
        return;
    }
    decision.needs_more_workers = 0;
    decision.actions.clear();
}

pub(crate) fn persist_thread(
    inner: &mut Inner,
    key: String,
    now: Instant,
    should_reassess: bool,
    previous_last_reassessed: Instant,
    active_units: HashSet<String>,
) {
    inner.threads.insert(
        key,
        core::LiveThreadState {
            last_seen: now,
            last_reassessed: if should_reassess {
                now
            } else {
                previous_last_reassessed
            },
            active_units,
        },
    );
    inner
        .threads
        .retain(|_, state| now.duration_since(state.last_seen) < Duration::from_secs(3600));
    if inner.threads.len() > 1_024 {
        inner.threads.clear();
    }
}

pub(crate) fn estimate_target_workers(
    snapshot: &core::SubagentSnapshot,
    request: &MessagesRequest,
    config: &SchedulerConfig,
) -> usize {
    let active = snapshot.active_count();
    if scope_count::declines_delegation(request) {
        return 0;
    }
    // Reconstructed transcripts without a classifiable user turn must not invent
    // fan-out; floor/replenishment owns those assistant-only states.
    if !scope_count::has_classifiable_user_turn(request) {
        return 0;
    }
    if scope_count::needs_single_worker(request) {
        return 1;
    }
    if scope_count::is_substantive_work(request) {
        let scopes = independent_scope_count(request).max(1);
        // Match independent scopes. A single scope stays at one worker even
        // when min_parallel_workers is raised by env.
        return scopes.min(config.max_parallel_workers);
    }
    if active > 0 {
        return 1;
    }
    0
}

pub(crate) fn scope_guidance(request: &MessagesRequest, decision: &SchedulerDecision) -> String {
    if scope_count::declines_delegation(request) {
        return "Task-shape: the user requested no new delegation. Do not launch another SubAgent."
            .to_owned();
    }
    if scope_count::needs_single_worker(request) {
        return "Task-shape: one bounded or indivisible lookup detected. Launch exactly one ordinary SubAgent; do not fan out.".to_owned();
    }
    if scope_count::is_substantive_work(request) {
        let inferred_count = independent_scope_count(request);
        if inferred_count >= 2 {
            let bounded_count = decision.target_workers.max(1);
            return format!(
                "Task-shape: multiple independent scopes detected. Launch exactly {bounded_count} ordinary SubAgents in the same assistant turn; do not stop after the first worker. Do not blindly launch the concurrent cap."
            );
        }
        return "Task-shape: one independent scope. Launch exactly one ordinary background Agent/Task worker; do not apply a minimum-parallel floor.".to_owned();
    }
    if !has_parallel_scope(request) {
        if decision.active_workers > 1 {
            return "Task-shape: one bounded scope detected. Keep exactly one ordinary SubAgent and stop duplicate same-scope workers.".to_owned();
        }
        return "Task-shape: one bounded or indivisible scope detected. Launch exactly one ordinary SubAgent.".to_owned();
    }
    "Task-shape: no parallel fan-out required.".to_owned()
}
