//! Provider quota normalization and capacity sort keys.

use super::{CapacityKey, FIVE_HOUR_WINDOW, SEVEN_DAY_WINDOW};
use crate::opencode_go_budget;
use crate::util::{number_f64, python_round, valid_percentage};
use anyhow::Result;
use serde_json::{Map, Value};

pub fn usage_percentages(value: &Value) -> Vec<f64> {
    match value {
        Value::Object(map) => object_usage_percentages(map),
        Value::Array(items) => items.iter().flat_map(usage_percentages).collect(),
        _ => Vec::new(),
    }
}

fn object_usage_percentages(map: &Map<String, Value>) -> Vec<f64> {
    let mut percentages = Vec::new();
    for (key, nested) in map {
        if key == "usedPercent" && valid_percentage(nested) {
            percentages.extend(number_f64(nested));
        } else {
            percentages.extend(usage_percentages(nested));
        }
    }
    percentages
}

pub fn find_report_entry<'a>(report: &'a Value, provider: Option<&str>) -> Option<&'a Value> {
    let provider = provider?;
    let entries = report.as_array()?;
    entries.iter().find(|item| {
        item.as_object().is_some_and(|object| {
            object
                .get("provider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(provider))
        })
    })
}

pub fn status(available: bool, maximum: Option<f64>, reason: &str) -> Value {
    serde_json::json!({
        "available": available,
        "max_used_percent": maximum,
        "remaining_percent": maximum.map(|value| (100.0 - value).max(0.0)),
        "reason": reason,
    })
}

pub fn explicitly_reported_status(entry: &Value) -> Value {
    let Some(object) = entry.as_object() else {
        return status(false, None, "unknown");
    };
    let Some(available) = object.get("available").and_then(Value::as_bool) else {
        return status(false, None, "unknown");
    };
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map_or_else(|| default_reason(available).to_owned(), str::to_owned);
    match object.get("maxUsedPercent") {
        None => status(available, None, &reason),
        Some(maximum) => {
            if !valid_percentage(maximum) || number_f64(maximum).is_some_and(|n| n > 100.0) {
                status(false, None, "unknown")
            } else {
                status(available, number_f64(maximum), &reason)
            }
        }
    }
}

fn default_reason(available: bool) -> &'static str {
    if available {
        "available"
    } else {
        "usage-unavailable"
    }
}

pub fn provider_status(report: &Value, provider: &str) -> Value {
    let Some(entry) = find_report_entry(report, Some(provider)) else {
        return status(false, None, "missing");
    };
    if entry.get("available").is_some() {
        return explicitly_reported_status(entry);
    }
    let percentages = usage_percentages(entry.get("usage").unwrap_or(&Value::Null));
    if percentages.is_empty() {
        return status(false, None, "unknown");
    }
    let maximum = percentages.into_iter().fold(f64::NEG_INFINITY, f64::max);
    status(
        maximum < 100.0,
        Some(maximum),
        if maximum < 100.0 {
            "available"
        } else {
            "exhausted"
        },
    )
}

pub fn quota_window_remaining(entry: Option<&Value>) -> Value {
    let mut remaining = serde_json::json!({
        FIVE_HOUR_WINDOW: Value::Null,
        SEVEN_DAY_WINDOW: Value::Null,
    });
    let Some(windows) = entry
        .and_then(|value| value.get("quotaWindows"))
        .and_then(Value::as_array)
    else {
        return remaining;
    };
    for window in windows {
        if let Some((name, percent)) = named_remaining_percent(window) {
            remaining[name] = Value::from(percent);
        }
    }
    remaining
}

fn named_remaining_percent(window: &Value) -> Option<(&str, f64)> {
    let object = window.as_object()?;
    let name = object.get("name").and_then(Value::as_str)?;
    if name != FIVE_HOUR_WINDOW && name != SEVEN_DAY_WINDOW {
        return None;
    }
    let value = object.get("remainingPercent")?;
    if !valid_percentage(value) {
        return None;
    }
    number_f64(value)
        .filter(|percent| *percent <= 100.0)
        .map(|percent| (name, percent))
}

pub fn effective_window_remaining(quota: &Value) -> (Option<f64>, Option<f64>) {
    let windows = quota.get("quota_windows");
    let weekly = windows
        .and_then(|value| value.get(SEVEN_DAY_WINDOW))
        .and_then(number_f64);
    let mut five_hour = windows
        .and_then(|value| value.get(FIVE_HOUR_WINDOW))
        .and_then(number_f64);
    let weekly = weekly.or_else(|| {
        quota
            .get("max_used_percent")
            .and_then(number_f64)
            .map(|maximum| (100.0 - maximum).max(0.0))
    });
    // OpenCode Go requestBudget meters the five-hour window. When CodexBar
    // windows were not attached, reuse that remaining so dynamic selection
    // still sees the short window.
    if five_hour.is_none() {
        five_hour = request_budget_five_hour_remaining(quota);
    }
    (weekly, five_hour)
}

fn request_budget_five_hour_remaining(quota: &Value) -> Option<f64> {
    let budget = quota.get("request_budget")?;
    if !budget.get("known").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let window_minutes = budget.get("window_minutes").and_then(Value::as_i64)?;
    // Only treat the published ~5h request window as five-hour headroom.
    if window_minutes != 300 {
        return None;
    }
    quota.get("remaining_percent").and_then(number_f64)
}

/// Remaining headroom for dynamic model selection.
///
/// When a five-hour meter is present, the tighter of weekly and five-hour
/// governs ranking and automatic filtering so a depleted short window cannot
/// hide behind a fat weekly bucket.
pub fn selection_remaining(weekly: Option<f64>, five_hour: Option<f64>) -> Option<f64> {
    match (weekly, five_hour) {
        (Some(weekly), Some(five_hour)) => Some(weekly.min(five_hour)),
        (Some(weekly), None) => Some(weekly),
        (None, Some(five_hour)) => Some(five_hour),
        (None, None) => None,
    }
}

fn push_codexbar_window(windows: &mut Vec<Value>, name: &str, window: &Map<String, Value>) {
    let Some(used_value) = window.get("usedPercent") else {
        return;
    };
    if !valid_percentage(used_value) {
        return;
    }
    let Some(used) = number_f64(used_value) else {
        return;
    };
    let mut converted = serde_json::json!({
        "name": name,
        "usedPercent": used,
        "remainingPercent": python_round(100.0 - used, 6),
    });
    if let Some(reset_at) = window.get("resetsAt").and_then(Value::as_str)
        && reset_at.ends_with('Z')
        && let Ok(parsed) = crate::util::parse_utc_datetime(&Value::from(reset_at))
    {
        converted["resetAtMilliseconds"] = Value::from((parsed * 1000.0) as i64);
    }
    windows.push(converted);
}

fn extra_rate_window<'a>(
    usage: &'a Map<String, Value>,
    window_id: &str,
) -> Option<&'a Map<String, Value>> {
    usage
        .get("extraRateWindows")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|entry| {
            let object = entry.as_object()?;
            if object.get("id").and_then(Value::as_str) != Some(window_id) {
                return None;
            }
            object.get("window").and_then(Value::as_object)
        })
}

/// Fill the seven-day slot and report whether the requested weekly window
/// existed. A configured-but-absent `usageWeeklyWindowId` fails closed.
fn push_weekly_window(
    windows: &mut Vec<Value>,
    usage: &Map<String, Value>,
    weekly_window_id: Option<&str>,
) -> bool {
    let Some(window_id) = weekly_window_id else {
        if let Some(secondary) = usage.get("secondary").and_then(Value::as_object) {
            push_codexbar_window(windows, SEVEN_DAY_WINDOW, secondary);
        }
        return false;
    };
    let Some(window) = extra_rate_window(usage, window_id) else {
        return true;
    };
    push_codexbar_window(windows, SEVEN_DAY_WINDOW, window);
    false
}

/// Normalize CodexBar primary/secondary usage windows into the shared
/// five-hour / seven-day shape used for ranking (Claude, Qwen Cloud, …).
///
/// When `weekly_window_id` is set, the seven-day slot is taken from
/// `usage.extraRateWindows[id == weekly_window_id].window` instead of
/// `usage.secondary`. A missing id fails closed (available=false).
pub fn codexbar_window_quota_entry(
    entry: Option<&Value>,
    weekly_window_id: Option<&str>,
) -> Option<Value> {
    let entry = entry?;
    let provider = entry.get("provider").and_then(Value::as_str)?;
    let usage = entry.get("usage")?.as_object()?;
    let mut windows = Vec::new();
    if let Some(primary) = usage.get("primary").and_then(Value::as_object) {
        push_codexbar_window(&mut windows, FIVE_HOUR_WINDOW, primary);
    }
    if push_weekly_window(&mut windows, usage, weekly_window_id) {
        return Some(serde_json::json!({
            "provider": provider,
            "available": false,
            "reason": "usage-weekly-window-missing",
            "quotaWindows": windows,
        }));
    }
    if windows.is_empty() {
        return None;
    }
    let maximum = windows
        .iter()
        .filter_map(|window| number_f64(&window["usedPercent"]))
        .fold(f64::NEG_INFINITY, f64::max);
    let available = maximum < 100.0;
    Some(serde_json::json!({
        "provider": provider,
        "available": available,
        "reason": if available {
            format!("available-{provider}-quota")
        } else {
            "exhausted".to_owned()
        },
        "maxUsedPercent": maximum,
        "quotaWindows": windows,
    }))
}

pub fn provider_weekly_window_id(provider: &Value) -> Option<&str> {
    provider
        .get("usageWeeklyWindowId")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

pub fn provider_quota_status(report: &Value, provider: &Value) -> Result<Value> {
    let usage_provider = provider.get("usageProvider").and_then(Value::as_str);
    let Some(usage_provider) = usage_provider.filter(|text| !text.is_empty()) else {
        return Ok(status(true, None, "unmetered"));
    };
    if let Some(budget) = provider.get("requestBudget")
        && let Some(evaluated) = opencode_go_budget::evaluate(report, usage_provider, Some(budget))?
    {
        return Ok(evaluated);
    }
    let weekly_window_id = provider_weekly_window_id(provider);
    if let Some(entry) = find_report_entry(report, Some(usage_provider))
        && let Some(normalized) = codexbar_window_quota_entry(Some(entry), weekly_window_id)
    {
        return Ok(explicitly_reported_status(&normalized));
    }
    Ok(provider_status(report, usage_provider))
}

pub fn native_worker_quota(report: &Value, worker: &Value) -> Result<Value> {
    let usage_provider = worker.get("usageProvider").and_then(Value::as_str);
    let Some(usage_provider) = usage_provider.filter(|text| !text.is_empty()) else {
        return Ok(status(true, None, "unmetered"));
    };
    let mut synthetic = serde_json::json!({ "usageProvider": usage_provider });
    if let Some(weekly_window_id) = provider_weekly_window_id(worker) {
        synthetic["usageWeeklyWindowId"] = Value::from(weekly_window_id);
    }
    provider_quota_status(report, &synthetic)
}

/// Attach the normalized five-hour / seven-day windows to a quota status.
pub fn quota_with_windows(report: &Value, provider: &Value, native: bool) -> Result<Value> {
    let usage_provider = provider.get("usageProvider").and_then(Value::as_str);
    let entry = find_report_entry(report, usage_provider);
    let normalized = codexbar_window_quota_entry(entry, provider_weekly_window_id(provider));
    let mut quota = if native {
        native_worker_quota(report, provider)?
    } else {
        provider_quota_status(report, provider)?
    };
    quota["quota_windows"] = quota_window_remaining(normalized.as_ref().or(entry));
    Ok(quota)
}

pub fn capacity_priority(quota: &Value, config_index: i64) -> CapacityKey {
    if quota.get("reason").and_then(Value::as_str) == Some("unmetered") {
        return (1.0, 0.0, 0.0, 0.0, 0.0, 0.0, config_index);
    }
    let (weekly, five_hour) = effective_window_remaining(quota);
    let Some(remaining) = selection_remaining(weekly, five_hour) else {
        return (2.0, 0.0, 0.0, 0.0, 0.0, 0.0, config_index);
    };
    (
        0.0,
        -remaining,
        if five_hour.is_some() { 0.0 } else { 1.0 },
        -five_hour.unwrap_or(0.0),
        -weekly.unwrap_or(0.0),
        0.0,
        config_index,
    )
}

pub fn combined_capacity_priority(
    quota: &Value,
    concurrency: &Value,
    config_index: i64,
) -> CapacityKey {
    let unmetered = quota.get("reason").and_then(Value::as_str) == Some("unmetered");
    let (mut weekly, mut five_hour) = effective_window_remaining(quota);
    let parallel_used = parallel_used_percent(concurrency);
    if parallel_used != 0.0 {
        let parallel_remaining = 100.0 - parallel_used;
        weekly = Some(weekly.map_or(parallel_remaining, |value| value.min(parallel_remaining)));
        five_hour =
            Some(five_hour.map_or(parallel_remaining, |value| value.min(parallel_remaining)));
    }
    let remaining = selection_remaining(weekly, five_hour);
    let tier = if unmetered {
        1.0
    } else if remaining.is_none() {
        2.0
    } else {
        0.0
    };
    let health_unknown =
        if concurrency.get("reason").and_then(Value::as_str) == Some("daemon-health-unavailable") {
            1.0
        } else {
            0.0
        };
    (
        tier,
        if remaining.is_some() { 0.0 } else { 1.0 },
        -remaining.unwrap_or(0.0),
        if five_hour.is_some() { 0.0 } else { 1.0 },
        -five_hour.unwrap_or(0.0),
        health_unknown,
        config_index,
    )
}

fn parallel_used_percent(concurrency: &Value) -> f64 {
    let slots = (
        concurrency.get("active").and_then(Value::as_i64),
        concurrency.get("queued").and_then(Value::as_i64),
        concurrency.get("limit").and_then(Value::as_i64),
    );
    let (Some(active), Some(queued), Some(limit)) = slots else {
        return 0.0;
    };
    if limit == 0 {
        return 0.0;
    }
    100.0 * (active + queued) as f64 / limit as f64
}
