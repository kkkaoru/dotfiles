use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use crate::anthropic::MessagesRequest;

mod config;
mod core;
mod env;
mod policy;
mod scope_count;

#[allow(unused_imports)]
pub(crate) use config::{
    DEFAULT_MAX_PARALLEL_WORKERS, SUBAGENT_ACTIVE_FLOOR_ENV, SUBAGENT_CLEANUP_ON_EXIT_ENV,
    SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV, SUBAGENT_MAX_PARALLEL_ENV,
    SUBAGENT_MIN_MODEL_FAMILIES_ENV, SUBAGENT_MIN_PARALLEL_ENV,
    SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV, SUBAGENT_REEVALUATE_ON_COMPLETION_ENV,
    SUBAGENT_REUSE_ENV, SchedulerConfig,
};

#[derive(Clone, Debug)]
pub(crate) struct SchedulerDecision {
    pub(crate) target_workers: usize,
    pub(crate) active_workers: usize,
    pub(crate) completed_recently: usize,
    pub(crate) active_model_families: usize,
    pub(crate) needs_more_workers: usize,
    pub(crate) needs_model_diversity: bool,
    pub(crate) active_floor_breached: bool,
    pub(crate) actions: Vec<String>,
}

impl SchedulerDecision {
    fn no_action() -> Self {
        Self {
            target_workers: 0,
            active_workers: 0,
            completed_recently: 0,
            active_model_families: 0,
            needs_more_workers: 0,
            needs_model_diversity: false,
            active_floor_breached: false,
            actions: Vec::new(),
        }
    }

    fn has_work(&self) -> bool {
        self.active_workers > 0
    }

    fn guidance(&self, config: &SchedulerConfig) -> String {
        let floor = config.active_floor.max(2);
        if self.actions.is_empty() {
            return format!(
                "Dynamic parallel status: keep {} active lane(s) for the current independent work; re-check after each SubAgent completion or every {} minutes.",
                self.target_workers,
                config.reassess_interval.as_secs().div_ceil(60).max(1)
            );
        }
        let mut lines = Vec::with_capacity(1 + self.actions.len() + 2);
        lines.push(format!(
            "SubAgent floor policy: maintain at least {} active lanes during long-running work; target concurrency is {}.",
            floor,
            self.target_workers
        ));
        if self.completed_recently > 0 {
            lines.push(format!(
                "Worker-cycle: {completed} worker(s) completed on this turnset; for unfinished objectives, continue running SubAgents with follow-up context, re-issue same-scope tasks, then add expanded follow-up scope.",
                completed = self.completed_recently
            ));
        }
        if self.active_model_families < config.min_model_families {
            lines.push(format!(
                "Model-policy: ensure at least {} model families remain active.",
                config.min_model_families
            ));
        }
        lines.extend(
            self.actions
                .iter()
                .map(|action| format!("Action: {action}")),
        );
        lines.push(format!(
            "Current active lanes: {} (completed recently: {}).",
            self.active_workers, self.completed_recently
        ));
        lines.join("\n")
    }
}

#[derive(Debug)]
struct Inner {
    config: SchedulerConfig,
    threads: HashMap<String, core::LiveThreadState>,
}

pub(crate) struct ParallelScheduler {
    inner: Mutex<Inner>,
}

impl ParallelScheduler {
    pub(crate) fn shared() -> &'static Self {
        static SCHEDULER: OnceLock<ParallelScheduler> = OnceLock::new();
        SCHEDULER.get_or_init(|| Self::new(SchedulerConfig::parse()))
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::new(SchedulerConfig::default())
    }

    pub(crate) fn new(config: SchedulerConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                config,
                threads: HashMap::new(),
            }),
        }
    }

    pub(crate) fn config(&self) -> SchedulerConfig {
        self.inner
            .lock()
            .expect("parallel scheduler state")
            .config
            .clone()
    }

    pub(crate) fn decision_for_request(&self, request: &MessagesRequest) -> SchedulerDecision {
        let now = Instant::now();
        let snapshot = core::analyze_subagent_work(&request.messages);
        let key = core::thread_key(request);
        let config = self.config();
        let target_workers = policy::estimate_target_workers(&snapshot, request, &config);

        let (completed_recently, previous_last_reassessed) =
            self.previous_state(&key, &snapshot, now);
        let mut decision = SchedulerDecision {
            target_workers,
            active_workers: snapshot.active_count(),
            completed_recently,
            active_model_families: snapshot.active_model_families(),
            ..SchedulerDecision::no_action()
        };

        let mut inner = self.inner.lock().expect("parallel scheduler state");
        let should_reassess = policy::reassessment_due(&inner, &key, now, &config);
        policy::apply_reassessment_actions(
            &mut decision,
            &snapshot,
            request,
            &config,
            should_reassess,
        );
        policy::apply_replenishment_target(
            &mut decision,
            &snapshot,
            request,
            &config,
            should_reassess,
        );
        let effective_target = decision.target_workers;
        policy::apply_capacity_actions(&mut decision, effective_target, &config);
        policy::apply_floor_action(&mut decision, request, &config);
        policy::apply_diversity_action(&mut decision, request, &config);
        policy::apply_reuse_actions(&mut decision, request, &config);
        policy::clear_empty_decision(&mut decision, &snapshot);
        policy::persist_thread(
            &mut inner,
            key,
            now,
            should_reassess,
            previous_last_reassessed,
            snapshot.active_unit_ids,
        );
        decision
    }

    fn previous_state(
        &self,
        key: &str,
        snapshot: &core::SubagentSnapshot,
        now: Instant,
    ) -> (usize, Instant) {
        let inner = self.inner.lock().expect("parallel scheduler state");
        let Some(previous) = inner.threads.get(key) else {
            return (0, now);
        };
        let completed = core::previous_completed(
            previous,
            &snapshot.active_unit_ids,
            previous.active_units.len(),
        );
        (completed, previous.last_reassessed)
    }

    pub(crate) fn guidance_for_request(&self, request: &MessagesRequest) -> String {
        let decision = self.decision_for_request(request);
        let config = self.config();
        format!(
            "{}\n{}",
            policy::scope_guidance(request, &decision),
            decision.guidance(&config)
        )
    }
}

// These modules contain only scheduler assertions; production scheduling code
// remains covered by the integration and stream tests.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "parallel_scheduler/model_diversity_tests.rs"]
mod model_diversity_tests;
// Keep test orchestration out of production coverage totals; the shared gate
// measures the scheduler implementation above and its external call paths.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
