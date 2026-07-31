use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::anthropic::MessagesRequest;

mod core;
mod env;
mod policy;

pub(crate) const SUBAGENT_MIN_PARALLEL_ENV: &str = "CLAUDEX_SUBAGENT_MIN_PARALLEL";
pub(crate) const SUBAGENT_ACTIVE_FLOOR_ENV: &str = "CLAUDEX_SUBAGENT_ACTIVE_FLOOR";
pub(crate) const SUBAGENT_MAX_PARALLEL_ENV: &str = "CLAUDEX_SUBAGENT_MAX_PARALLEL";
pub(crate) const SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV: &str =
    "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS";
pub(crate) const SUBAGENT_REEVALUATE_ON_COMPLETION_ENV: &str =
    "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION";
pub(crate) const SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV: &str =
    "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS";
pub(crate) const SUBAGENT_MIN_MODEL_FAMILIES_ENV: &str = "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES";
pub(crate) const SUBAGENT_REUSE_ENV: &str = "CLAUDEX_SUBAGENT_REUSE";
pub(crate) const SUBAGENT_CLEANUP_ON_EXIT_ENV: &str = "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT";

const DEFAULT_MIN_PARALLEL_WORKERS: usize = 3;
const DEFAULT_ACTIVE_FLOOR: usize = 2;
const DEFAULT_MAX_PARALLEL_WORKERS: usize = 40;
const DEFAULT_MIN_MODEL_FAMILIES: usize = 2;
const DEFAULT_REASSESS_INTERVAL_SECONDS: u64 = 600;

#[derive(Clone, Debug)]
pub(crate) struct SchedulerConfig {
    pub(crate) min_parallel_workers: usize,
    pub(crate) max_parallel_workers: usize,
    pub(crate) active_floor: usize,
    pub(crate) reevaluate_on_completion: bool,
    pub(crate) reassess_interval: Duration,
    pub(crate) min_model_families: usize,
    pub(crate) allow_reuse: bool,
    pub(crate) cleanup_on_exit: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            min_parallel_workers: DEFAULT_MIN_PARALLEL_WORKERS,
            max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
            active_floor: DEFAULT_ACTIVE_FLOOR,
            reevaluate_on_completion: true,
            reassess_interval: Duration::from_secs(DEFAULT_REASSESS_INTERVAL_SECONDS),
            min_model_families: DEFAULT_MIN_MODEL_FAMILIES,
            allow_reuse: true,
            cleanup_on_exit: true,
        }
    }
}

impl SchedulerConfig {
    fn parse() -> Self {
        let mut config = Self::default();
        let max_workers = env::parse_usize_env(SUBAGENT_MAX_PARALLEL_ENV)
            .or_else(|| env::parse_usize_env(SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV));
        config.max_parallel_workers = max_workers.unwrap_or(DEFAULT_MAX_PARALLEL_WORKERS);
        config.max_parallel_workers = config
            .max_parallel_workers
            .clamp(DEFAULT_MIN_PARALLEL_WORKERS, DEFAULT_MAX_PARALLEL_WORKERS);
        if let Some(value) = env::parse_usize_env(SUBAGENT_MIN_PARALLEL_ENV) {
            config.min_parallel_workers = value;
        }
        if let Some(value) = env::parse_usize_env(SUBAGENT_ACTIVE_FLOOR_ENV) {
            config.active_floor = value;
        }
        if let Some(value) = env::parse_bool_env(SUBAGENT_REEVALUATE_ON_COMPLETION_ENV) {
            config.reevaluate_on_completion = value;
        }
        if let Some(value) = env::parse_u64_env(SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV) {
            config.reassess_interval = Duration::from_secs(value);
        }
        if let Some(value) = env::parse_usize_env(SUBAGENT_MIN_MODEL_FAMILIES_ENV) {
            config.min_model_families = value;
        }
        if let Some(value) = env::parse_bool_env(SUBAGENT_REUSE_ENV) {
            config.allow_reuse = value;
        }
        if let Some(value) = env::parse_bool_env(SUBAGENT_CLEANUP_ON_EXIT_ENV) {
            config.cleanup_on_exit = value;
        }
        let max_floor = config.max_parallel_workers.saturating_sub(1).max(2);
        config.active_floor = config.active_floor.clamp(2, max_floor);
        let min_workers = DEFAULT_MIN_PARALLEL_WORKERS.max(config.active_floor.saturating_add(1));
        config.min_parallel_workers = config
            .min_parallel_workers
            .clamp(min_workers, config.max_parallel_workers);
        config.active_floor = config
            .active_floor
            .clamp(2, config.min_parallel_workers - 1);
        config.min_model_families = config.min_model_families.max(2);
        config
    }
}

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
        policy::apply_reassessment_actions(&mut decision, &snapshot, &config, should_reassess);
        policy::apply_replenishment_target(&mut decision, &snapshot, &config, should_reassess);
        let effective_target = decision.target_workers;
        policy::apply_capacity_actions(&mut decision, effective_target, &config);
        policy::apply_floor_action(&mut decision, &config);
        policy::apply_diversity_action(&mut decision, &config);
        policy::apply_reuse_actions(&mut decision, &config);
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
        decision.guidance(&config)
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
