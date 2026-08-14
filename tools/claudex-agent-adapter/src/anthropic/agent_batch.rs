use serde_json::{Value, json};

const MAPPED_NAME_PREFIX: &str = "__claudex_agent_batch__:";
const MARKER_KEY: &str = "claudexAgentBatch";
const DEFAULT_MAX_BATCH_SIZE: usize = 40;

pub(super) fn minimum_batch_size() -> usize {
    effective_batch_range(&crate::parallel_scheduler::ParallelScheduler::shared().config()).0
}

pub(super) struct PendingBatch<'a> {
    pub(super) request_id: &'a Value,
    pub(super) index: usize,
    pub(super) total: usize,
}

pub(super) fn original_name(mapped: &str) -> Option<&str> {
    mapped.strip_prefix(MAPPED_NAME_PREFIX)
}

pub(super) fn maximum_batch_size() -> usize {
    let (_, maximum) =
        effective_batch_range(&crate::parallel_scheduler::ParallelScheduler::shared().config());
    maximum
}

fn effective_batch_range(config: &crate::parallel_scheduler::SchedulerConfig) -> (usize, usize) {
    let maximum = config.max_parallel_workers.clamp(1, DEFAULT_MAX_BATCH_SIZE);
    // Legacy batch schema prefers at least three tasks, but must never raise
    // the maximum to satisfy that floor when max_parallel is 1 or 2.
    let minimum = config.min_parallel_workers.max(3).min(maximum);
    (minimum, maximum)
}

pub(super) fn pending_marker(request_id: Value, index: usize, total: usize) -> Value {
    json!({MARKER_KEY:{"requestId":request_id,"index":index,"total":total}})
}

pub(super) fn pending_batch(value: &Value) -> Option<PendingBatch<'_>> {
    let marker = value.get(MARKER_KEY)?;
    Some(PendingBatch {
        request_id: marker.get("requestId")?,
        index: marker.get("index")?.as_u64()?.try_into().ok()?,
        total: marker.get("total")?.as_u64()?.try_into().ok()?,
    })
}

#[cfg(test)]
// Test-only assertions are excluded from production coverage by the shared
// coverage gate; the production batch validation remains covered through the
// session and stream tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn batch_minimum_stays_at_three_when_scheduler_min_is_one() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            min_parallel_workers: 1,
            ..Default::default()
        };
        let (minimum, _) = effective_batch_range(&config);
        assert_eq!(minimum, 3);
    }

    #[test]
    fn caps_batch_max_to_effective_maximum() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            max_parallel_workers: 7,
            min_parallel_workers: 3,
            ..Default::default()
        };
        let (minimum, maximum) = effective_batch_range(&config);
        assert_eq!(minimum, 3);
        assert_eq!(maximum, 7);
    }

    #[test]
    fn clamps_batch_max_to_internal_hard_limit() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            max_parallel_workers: 99,
            min_parallel_workers: 3,
            ..Default::default()
        };
        let (_, maximum) = effective_batch_range(&config);
        assert_eq!(maximum, 40);
    }

    #[test]
    fn keeps_schema_max_at_least_as_large_as_schema_minimum() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            max_parallel_workers: 3,
            min_parallel_workers: 8,
            ..Default::default()
        };
        let (minimum, maximum) = effective_batch_range(&config);
        assert_eq!(minimum, 3);
        assert_eq!(maximum, 3);
        assert!(minimum <= maximum);
    }

    #[test]
    fn batch_minimum_does_not_bypass_max_one() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            min_parallel_workers: 1,
            max_parallel_workers: 1,
            ..Default::default()
        };
        let (minimum, maximum) = effective_batch_range(&config);
        assert_eq!(minimum, 1);
        assert_eq!(maximum, 1);
    }

    #[test]
    fn batch_minimum_does_not_bypass_max_two() {
        let config = crate::parallel_scheduler::SchedulerConfig {
            min_parallel_workers: 1,
            max_parallel_workers: 2,
            ..Default::default()
        };
        let (minimum, maximum) = effective_batch_range(&config);
        assert_eq!(minimum, 2);
        assert_eq!(maximum, 2);
        assert!(minimum <= maximum);
    }
}
