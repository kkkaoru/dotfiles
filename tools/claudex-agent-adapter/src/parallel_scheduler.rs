use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

#[cfg(test)]
use crate::anthropic::MessagesRequest;

mod config;
mod core;
mod decide;
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
pub(crate) use scope_count::{
    has_classifiable_user_turn, has_parallel_scope, is_substantive_work, needs_single_worker,
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

mod decision;

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
