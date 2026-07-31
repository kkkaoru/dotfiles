use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::anthropic::MessagesRequest;
use serde_json::Value;

use super::{SchedulerConfig, SchedulerDecision, core};

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
    decision.target_workers = decision
        .target_workers
        .max(config.active_floor)
        .min(config.max_parallel_workers);
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

pub(crate) fn has_parallel_scope(request: &MessagesRequest) -> bool {
    independent_scope_count(request) >= 2
}

fn has_explicit_parallel_scope(request: &MessagesRequest) -> bool {
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .any(|content| count_explicit_blocks(content) >= 2 || contains_parallel_intent(content))
}

pub(crate) fn independent_scope_count(request: &MessagesRequest) -> usize {
    let user_text = request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let Some(content) = user_text.last() else {
        // A reconstructed request without a user turn cannot be classified safely. Keep the
        // existing conservative behavior until the next user turn supplies a scope.
        return 2;
    };
    if contains_single_scope_request(content) {
        return 1;
    }
    let explicit_blocks = count_explicit_blocks(content);
    if explicit_blocks >= 2 {
        return explicit_blocks;
    }
    if contains_parallel_intent(content) {
        2
    } else {
        1
    }
}

fn contains_single_scope_request(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    [
        "exactly one",
        "one worker",
        "one subagent",
        "1つのsubagent",
        "1つのsub agent",
        "単一",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

fn contains_parallel_intent(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    [
        "parallel",
        "multiple",
        "independent",
        "in parallel",
        "複数",
        "並列",
        "分担",
        "各観点",
        "比較",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

pub(crate) fn apply_diversity_action(
    decision: &mut SchedulerDecision,
    request: &MessagesRequest,
    config: &SchedulerConfig,
) {
    if !has_parallel_scope(request)
        || decision.active_workers == 0
        || decision.active_model_families >= config.min_model_families
    {
        return;
    }
    decision.needs_model_diversity = true;
    decision.actions.push(format!(
        "Diversify providers: active models should cover at least {} families.",
        config.min_model_families
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
            "Prefer reusing compatible completed workers via SendMessage; add new tasks to the same workers when their context fits."
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
            "After completion, reclaim obsolete or idle lanes; maintain two+ active reusable workers to sustain throughput."
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
    inner: &mut super::Inner,
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
    let requested_scopes = independent_scope_count(request);
    let active = snapshot.active_count();
    // A reconstructed request can contain only an assistant-side advisor call and no
    // user scope.  Do not turn that metadata-only state into ordinary worker launches.
    if active == 0
        && !request
            .messages
            .iter()
            .any(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    {
        return 0;
    }
    let mut target = active;
    if target == 0 && needs_single_worker(request) {
        target = 1;
    }
    if requested_scopes >= 2 {
        target = target.max(requested_scopes);
        if has_explicit_parallel_scope(request) {
            target = target.max(config.min_parallel_workers);
        }
        target = target.max(snapshot.active_model_families().saturating_add(1));
    }
    target.min(config.max_parallel_workers)
}

fn needs_single_worker(request: &MessagesRequest) -> bool {
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .any(|content| {
            let normalized = content.to_ascii_lowercase();
            [
                "gh ",
                "git ",
                "bash",
                "shell",
                "command",
                "http://",
                "https://",
                "調査",
                "取得",
                "確認",
                "実行",
                "修正",
                "テスト",
                "review",
                "research",
                "investigate",
                "fetch",
                "implement",
                "fix",
                "test",
            ]
            .iter()
            .any(|hint| normalized.contains(hint))
        })
}

pub(crate) fn scope_guidance(
    request: &MessagesRequest,
    decision: &SchedulerDecision,
) -> &'static str {
    if !has_parallel_scope(request) {
        if decision.active_workers > 1 {
            return "Task-shape: one bounded scope detected. Keep exactly one ordinary SubAgent and stop duplicate same-scope workers; selected_workers is a capacity pool, not a launch count.";
        }
        return "Task-shape: one bounded or indivisible scope detected. Launch exactly one ordinary SubAgent; selected_workers is a capacity pool, not a launch count.";
    }
    "Task-shape: multiple independent scopes detected. Launch only the number of non-redundant workers justified by those scopes, then reassess as each completes."
}

fn count_explicit_blocks(content: &str) -> usize {
    content
        .lines()
        .filter(|line| is_explicit_block(line))
        .count()
}

fn is_explicit_block(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("・")
        || is_numbered_block(trimmed)
}

fn is_numbered_block(trimmed: &str) -> bool {
    let Some(character) = trimmed.chars().next() else {
        return false;
    };
    if !character.is_ascii_digit() {
        return false;
    }
    trimmed
        .char_indices()
        .nth(1)
        .is_some_and(|(index, _)| trimmed[index..].starts_with(". "))
}
