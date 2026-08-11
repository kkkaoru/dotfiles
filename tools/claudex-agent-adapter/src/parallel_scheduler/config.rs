use std::time::Duration;

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
pub(crate) const DEFAULT_MAX_PARALLEL_WORKERS: usize = 40;
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
    pub(super) fn parse() -> Self {
        let mut config = Self::default();
        let max_workers = super::env::parse_usize_env(SUBAGENT_MAX_PARALLEL_ENV)
            .or_else(|| super::env::parse_usize_env(SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV));
        config.max_parallel_workers = max_workers.unwrap_or(DEFAULT_MAX_PARALLEL_WORKERS);
        config.max_parallel_workers = config
            .max_parallel_workers
            .clamp(DEFAULT_MIN_PARALLEL_WORKERS, DEFAULT_MAX_PARALLEL_WORKERS);
        if let Some(value) = super::env::parse_usize_env(SUBAGENT_MIN_PARALLEL_ENV) {
            config.min_parallel_workers = value;
        }
        if let Some(value) = super::env::parse_usize_env(SUBAGENT_ACTIVE_FLOOR_ENV) {
            config.active_floor = value;
        }
        if let Some(value) = super::env::parse_bool_env(SUBAGENT_REEVALUATE_ON_COMPLETION_ENV) {
            config.reevaluate_on_completion = value;
        }
        if let Some(value) = super::env::parse_u64_env(SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV) {
            config.reassess_interval = Duration::from_secs(value);
        }
        if let Some(value) = super::env::parse_usize_env(SUBAGENT_MIN_MODEL_FAMILIES_ENV) {
            config.min_model_families = value;
        }
        if let Some(value) = super::env::parse_bool_env(SUBAGENT_REUSE_ENV) {
            config.allow_reuse = value;
        }
        if let Some(value) = super::env::parse_bool_env(SUBAGENT_CLEANUP_ON_EXIT_ENV) {
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

