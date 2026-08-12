//! Capacity ranking, worker selection, concurrency refresh, and orchestration.

pub mod concurrency;
pub mod orchestration;
pub mod quota;
pub mod summary;
pub mod workers;

#[cfg(test)]
mod tests;

use anyhow::Result;
use serde_json::Value;
use std::env;

pub const FIVE_HOUR_WINDOW: &str = "five-hour";
pub const SEVEN_DAY_WINDOW: &str = "seven-day";
pub const DEFAULT_MAX_SUBAGENTS: i64 = 40;
/// No launch floor: `task_fanout` matches independent scopes (one scope → one
/// worker). Env `CLAUDEX_SUBAGENT_MIN_PARALLEL` can raise this; do not default
/// to 3 ordinary workers on a single question.
pub const DEFAULT_MIN_SUBAGENTS_PER_PHASE: i64 = 1;
pub const DEFAULT_ACTIVE_SUBAGENT_FLOOR: i64 = 1;
pub const DEFAULT_MIN_MODEL_KINDS: i64 = 1;
pub const ORCHESTRATION_REBALANCE_INTERVAL_SECONDS: i64 = 10 * 60;
pub const DEFAULT_SUBAGENT_STATUS_POLL_SECONDS: i64 = 15;
pub const SUBAGENT_MAX_PARALLEL_ENV: &str = "CLAUDEX_SUBAGENT_MAX_PARALLEL";
pub const SUBAGENT_MIN_PARALLEL_ENV: &str = "CLAUDEX_SUBAGENT_MIN_PARALLEL";
pub const SUBAGENT_ACTIVE_FLOOR_ENV: &str = "CLAUDEX_SUBAGENT_ACTIVE_FLOOR";
pub const SUBAGENT_MIN_MODEL_FAMILIES_ENV: &str = "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES";
pub const SUBAGENT_REEVALUATE_ON_COMPLETION_ENV: &str = "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION";
pub const SUBAGENT_REASSESS_INTERVAL_ENV: &str = "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS";
pub const SUBAGENT_REUSE_ENV: &str = "CLAUDEX_SUBAGENT_REUSE";
pub const SUBAGENT_CLEANUP_ON_EXIT_ENV: &str = "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT";
pub const SUBAGENT_FIRST_ENV: &str = "CLAUDEX_SUBAGENT_FIRST";
pub const SUBAGENT_STATUS_POLL_ENV: &str = "CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS";
pub const CUSTOM_ADVISOR_ENV: &str = "CLAUDEX_CUSTOM_ADVISOR";
pub const MEMORY_MANAGEMENT_ENV: &str = "CLAUDEX_MEMORY_MANAGEMENT";
pub const MEMORY_AVAILABLE_PCT_CRITICAL_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_CRITICAL";
pub const MEMORY_AVAILABLE_PCT_LOW_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_LOW";
pub const MEMORY_AVAILABLE_PCT_MEDIUM_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_MEDIUM";
pub const MEMORY_AVAILABLE_PCT_MODERATE_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_MODERATE";
pub const DEFAULT_MEMORY_AVAILABLE_PCT_CRITICAL: f64 = 10.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_LOW: f64 = 20.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_MEDIUM: f64 = 30.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_MODERATE: f64 = 40.0;

pub const CUSTOM_ADVISOR_CONSULT_WHEN: &[&str] = &[
    "high_risk_implementation_or_config_change",
    "worker_failure_timeout_or_stall",
    "conflicting_worker_results",
];

pub type CapacityKey = (f64, f64, f64, f64, f64, f64, i64);

/// An orchestration switch that stays on unless explicitly disabled.
fn switch_enabled(name: &str) -> bool {
    match env::var(name) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off")
        }
        Err(_) => true,
    }
}

pub fn custom_advisor_enabled() -> bool {
    switch_enabled(CUSTOM_ADVISOR_ENV)
}

pub fn memory_management_enabled() -> bool {
    switch_enabled(MEMORY_MANAGEMENT_ENV)
}

fn memory_fraction_env(name: &str, default: f64) -> Result<f64> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) => {
            let value: f64 = raw
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} must be a number between 0 and 100"))?;
            if !(0.0..=100.0).contains(&value) {
                anyhow::bail!("{name} must be a number between 0 and 100");
            }
            Ok(value)
        }
    }
}

pub fn memory_pressure_thresholds() -> Result<(f64, f64, f64, f64)> {
    let thresholds = (
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_CRITICAL_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_CRITICAL,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_LOW_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_LOW,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_MEDIUM_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_MEDIUM,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_MODERATE_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_MODERATE,
        )?,
    );
    let sorted = {
        let mut values = [thresholds.0, thresholds.1, thresholds.2, thresholds.3];
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values
    };
    if [thresholds.0, thresholds.1, thresholds.2, thresholds.3] != sorted {
        anyhow::bail!("memory thresholds must be ascending: critical <= low <= medium <= moderate");
    }
    Ok(thresholds)
}

pub fn pressure_level(available_percent: f64, thresholds: (f64, f64, f64, f64)) -> &'static str {
    let (critical, low, medium, moderate) = thresholds;
    if available_percent < critical {
        "critical"
    } else if available_percent < low {
        "high"
    } else if available_percent < medium {
        "medium"
    } else if available_percent < moderate {
        "moderate"
    } else {
        "ok"
    }
}

/// Memory pressure caps for `max_parallel_workers`. Kept generous so moderate
/// RAM pressure still allows multi-scope fan-out (was 1/2/4/8; now 2/6/16/32).
pub fn memory_parallel_cap(
    available_percent: f64,
    thresholds: (f64, f64, f64, f64),
) -> Option<i64> {
    match pressure_level(available_percent, thresholds) {
        "critical" => Some(2),
        "high" => Some(6),
        "medium" => Some(16),
        "moderate" => Some(32),
        _ => None,
    }
}

/// Order capacity candidates best-first and drop the sort keys.
pub fn rank_selected_workers(mut candidates: Vec<(CapacityKey, Value)>) -> Vec<Value> {
    candidates.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.into_iter().map(|(_, item)| item).collect()
}
