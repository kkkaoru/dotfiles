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
        return Ok(Some(status(
            false,
            None,
            "missing",
            &normalized,
            Map::new(),
        )));
    };
    let window_name = normalized["usageWindow"].as_str().unwrap_or_default();
    let window = entry
        .get("usage")
        .and_then(Value::as_object)
        .and_then(|usage| usage.get(window_name));
    let Some(window) = window.filter(|value| value.is_object()) else {
        return Ok(Some(status(
            false,
            None,
            "request-budget-window-missing",
            &normalized,
            Map::new(),
        )));
    };
    let used_percent = window.get("usedPercent");
    let reported_minutes = window.get("windowMinutes");
    if used_percent.is_none_or(|value| !valid_budget_percent(value)) {
        return Ok(Some(status(
            false,
            None,
            "request-budget-usage-unknown",
            &normalized,
            Map::new(),
        )));
    }
    let expected_minutes = normalized["windowMinutes"].as_i64();
    let reported_ok = reported_minutes
        .filter(|value| !value.is_boolean())
        .and_then(Value::as_i64)
        .zip(expected_minutes)
        .is_some_and(|(reported, expected)| reported == expected);
    if !reported_ok {
        let mut details = Map::new();
        details.insert(
            "reported_window_minutes".into(),
            reported_minutes.cloned().unwrap_or(Value::Null),
        );
        return Ok(Some(status(
            false,
            None,
            "request-budget-window-mismatch",
            &normalized,
            details,
        )));
    }
    let percent = number_f64(used_percent.unwrap()).unwrap_or_default();
    let total = normalized["estimatedRequests"].as_f64().unwrap_or_default();
    let estimated_used = python_round(total * percent / 100.0, 3);
    let estimated_remaining = python_round((total - estimated_used).max(0.0), 3);
    let reset_at = window
        .get("resetsAt")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map_or(Value::Null, Value::from);
    let mut details = Map::new();
    details.insert(
        "reported_window_minutes".into(),
        reported_minutes.cloned().unwrap_or(Value::Null),
    );
    details.insert(
        "estimated_used_requests".into(),
        Value::from(estimated_used),
    );
    details.insert(
        "estimated_remaining_requests".into(),
        Value::from(estimated_remaining),
    );
    details.insert("resets_at".into(), reset_at);
    Ok(Some(status(
        percent < 100.0,
        Some(percent),
        if percent < 100.0 {
            "available"
        } else {
            "request-budget-exhausted"
        },
        &normalized,
        details,
    )))
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
