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
fn error_flow_no_retry_uses_provider_neutral_label() {
    let event = json!({"params":{"willRetry":false}});
    let error = super::error_flow(&event).expect_err("willRetry:false must fail");
    assert_eq!(
        error.to_string(),
        "ACP provider turn failed: {\"willRetry\":false}"
    );
}

#[test]
fn error_flow_grok_balance_402_with_no_retry_is_terminal() {
    let event = json!({
        "params": {
            "willRetry": false,
            "error": {
                "message": "Configured ACP prompt failed: http_status:402 Payment Required: usage balance exhausted"
            }
        }
    });
    let error = super::error_flow(&event).expect_err("willRetry:false must not loop on Grok 402");
    assert!(error.to_string().contains("usage balance exhausted"));
}

#[test]
fn error_flow_missing_will_retry_fails() {
    let event = json!({"params":{}});
    let result = super::error_flow(&event);
    assert!(result.is_err());
}

#[test]
fn error_flow_labels_cline_credits_without_codex_wrap() {
    let event = json!({
        "params": {
            "willRetry": false,
            "error": {
                "message": "ConfiguredLaunch ACP prompt failed: Internal error: Insufficient balance. Add credits at https://app.cline.bot/credits"
            }
        }
    });
    let error = super::error_flow(&event).expect_err("cline credits is terminal");
    let message = error.to_string();
    assert!(message.contains("Cline Credits"), "{message}");
    assert!(message.contains("Do not retry"), "{message}");
    assert!(
        !message.contains("codex app-server turn failed"),
        "{message}"
    );
}
