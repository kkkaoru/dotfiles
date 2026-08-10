use serde_json::json;

#[test]
fn turn_flow_completed_breaks() {
    let event = json!({"params":{"turn":{"status":"completed"}}});
    let result = super::turn_flow(&event);
    assert!(result.is_ok());
    assert!(result.unwrap().is_break());
}

#[test]
fn turn_flow_null_breaks() {
    let event = json!({"params":{"turn":{}}});
    let result = super::turn_flow(&event);
    assert!(result.is_ok());
    assert!(result.unwrap().is_break());
}

#[test]
fn turn_flow_in_progress_continues() {
    let event = json!({"params":{"turn":{"status":"inProgress"}}});
    let result = super::turn_flow(&event);
    assert!(result.is_ok());
    assert!(result.unwrap().is_continue());
}

#[test]
fn turn_flow_unknown_status_fails() {
    let event = json!({"params":{"turn":{"status":"unknown"}}});
    let result = super::turn_flow(&event);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown"));
}

#[test]
fn error_flow_will_retry_continues() {
    let event = json!({"params":{"willRetry":true}});
    let result = super::error_flow(&event);
    assert!(result.is_ok());
    assert!(result.unwrap().is_continue());
}

#[test]
fn error_flow_no_retry_fails() {
    let event = json!({"params":{"willRetry":false}});
    let result = super::error_flow(&event);
    assert!(result.is_err());
}

#[test]
fn error_flow_missing_will_retry_fails() {
    let event = json!({"params":{}});
    let result = super::error_flow(&event);
    assert!(result.is_err());
}
