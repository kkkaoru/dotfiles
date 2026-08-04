//! Evaluate the published OpenCode Go request budget for one usage window.

use crate::util::{number_f64, python_round, valid_percentage};
use serde_json::{Map, Value};

pub fn evaluate(
    report: &Value,
    usage_provider: &str,
    budget: Option<&Value>,
) -> Result<Option<Value>, anyhow::Error> {
    let Some(budget_value) = budget else {
        return Ok(None);
    };
    let Some(normalized) = normalized_request_budget(budget_value) else {
        anyhow::bail!("invalid OpenCode Go request budget");
    };
    let Some(entry) = find_entry(report, usage_provider) else {
        return Ok(Some(unknown_status("missing", &normalized)));
    };
    let window_name = normalized["usageWindow"].as_str().unwrap_or_default();
    let Some(window) = budget_window(entry, window_name) else {
        return Ok(Some(unknown_status(
            "request-budget-window-missing",
            &normalized,
        )));
    };
    Ok(Some(evaluate_window(window, &normalized)))
}

fn unknown_status(reason: &str, budget: &Map<String, Value>) -> Value {
    status(false, None, reason, budget, Map::new())
}

fn budget_window<'a>(entry: &'a Value, window_name: &str) -> Option<&'a Value> {
    entry
        .get("usage")
        .and_then(Value::as_object)
        .and_then(|usage| usage.get(window_name))
        .filter(|value| value.is_object())
}

fn evaluate_window(window: &Value, budget: &Map<String, Value>) -> Value {
    let reported_minutes = window.get("windowMinutes");
    let Some(used_percent) = window
        .get("usedPercent")
        .filter(|value| valid_budget_percent(value))
    else {
        return unknown_status("request-budget-usage-unknown", budget);
    };
    let mut details = Map::new();
    details.insert(
        "reported_window_minutes".into(),
        reported_minutes.cloned().unwrap_or(Value::Null),
    );
    if !reported_window_matches(reported_minutes, budget["windowMinutes"].as_i64()) {
        return status(
            false,
            None,
            "request-budget-window-mismatch",
            budget,
            details,
        );
    }
    let percent = number_f64(used_percent).unwrap_or_default();
    let total = budget["estimatedRequests"].as_f64().unwrap_or_default();
    let estimated_used = python_round(total * percent / 100.0, 3);
    let estimated_remaining = python_round((total - estimated_used).max(0.0), 3);
    details.insert(
        "estimated_used_requests".into(),
        Value::from(estimated_used),
    );
    details.insert(
        "estimated_remaining_requests".into(),
        Value::from(estimated_remaining),
    );
    details.insert("resets_at".into(), reset_at(window));
    status(
        percent < 100.0,
        Some(percent),
        if percent < 100.0 {
            "available"
        } else {
            "request-budget-exhausted"
        },
        budget,
        details,
    )
}

fn reported_window_matches(reported: Option<&Value>, expected: Option<i64>) -> bool {
    reported
        .filter(|value| !value.is_boolean())
        .and_then(Value::as_i64)
        .zip(expected)
        .is_some_and(|(reported, expected)| reported == expected)
}

fn reset_at(window: &Value) -> Value {
    window
        .get("resetsAt")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map_or(Value::Null, Value::from)
}

fn normalized_request_budget(value: &Value) -> Option<Map<String, Value>> {
    if !crate::config::valid_request_budget(value) {
        return None;
    }
    let object = value.as_object()?;
    let mut out = Map::new();
    out.insert(
        "estimatedRequests".into(),
        Value::from(object["estimatedRequests"].as_i64().unwrap_or_default()),
    );
    out.insert(
        "windowMinutes".into(),
        Value::from(object["windowMinutes"].as_i64().unwrap_or_default()),
    );
    out.insert(
        "usageWindow".into(),
        Value::from(object["usageWindow"].as_str().unwrap_or_default()),
    );
    Some(out)
}

fn find_entry<'a>(report: &'a Value, provider: &str) -> Option<&'a Value> {
    let entries = report.as_array()?;
    entries.iter().find(|item| {
        item.get("provider")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case(provider))
    })
}

fn valid_budget_percent(value: &Value) -> bool {
    valid_percentage(value) && number_f64(value).is_some_and(|n| n <= 100.0)
}

fn status(
    available: bool,
    used_percent: Option<f64>,
    reason: &str,
    budget: &Map<String, Value>,
    details: Map<String, Value>,
) -> Value {
    let mut request_budget = Map::new();
    request_budget.insert(
        "estimated_requests".into(),
        budget["estimatedRequests"].clone(),
    );
    request_budget.insert("window_minutes".into(), budget["windowMinutes"].clone());
    request_budget.insert("usage_window".into(), budget["usageWindow"].clone());
    request_budget.insert("known".into(), Value::from(used_percent.is_some()));
    request_budget.insert(
        "used_percent".into(),
        used_percent.map_or(Value::Null, Value::from),
    );
    for (key, value) in details {
        request_budget.insert(key, value);
    }
    serde_json::json!({
        "available": available,
        "max_used_percent": used_percent,
        "remaining_percent": used_percent.map(|used| (100.0 - used).max(0.0)),
        "reason": reason,
        "request_budget": request_budget,
    })
}
