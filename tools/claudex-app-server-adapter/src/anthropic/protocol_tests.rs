use std::ops::ControlFlow;

use serde_json::json;

use super::{
    stream::{error_flow, message_start, tool_use_frames, turn_flow},
    subscription::{subscription_result, validate_subscription_result},
    subscription_stream::{result_output_tokens, subscription_start_frame},
};

#[test]
fn builds_subscription_and_stream_protocol_frames() {
    assert_eq!(
        subscription_result(br#"{"subtype":"success","is_error":false,"result":"OK"}"#).unwrap(),
        "OK"
    );
    assert!(subscription_result(b"not-json").is_err());
    assert!(subscription_result(br#"{"type":"result"}"#).is_err());
    assert!(subscription_start_frame("model", 12).contains("message_start"));
    let successful = json!({
        "subtype":"success", "is_error":false, "result":"hello",
        "usage":{"output_tokens":7}
    });
    validate_subscription_result(&successful).expect("successful subscription result");
    assert_eq!(result_output_tokens(&successful), 7);
    assert_eq!(result_output_tokens(&json!({"result":"12345"})), 2);
    assert!(validate_subscription_result(&json!({"subtype":"error","result":"bad"})).is_err());
    assert!(
        subscription_result(
            br#"{"subtype":"error","is_error":true,"result":"subscription failed"}"#
        )
        .unwrap_err()
        .to_string()
        .contains("subscription failed")
    );
    assert!(message_start("model", 12).contains("input_tokens"));

    let block = json!({
        "id":"toolu_test", "name":"lookup", "input":{"key":"value"}
    });
    let tool_frames = tool_use_frames(2, &block);
    assert_eq!(tool_frames.len(), 3);
    assert_eq!(tool_frames[0].1["index"], 2);
    assert!(tool_frames[1].1["delta"]["partial_json"].as_str().is_some());
}

#[test]
fn classifies_turn_and_retry_events() {
    assert_eq!(
        turn_flow(&json!({"params":{"turn":{"status":"completed"}}})).unwrap(),
        ControlFlow::Break(())
    );
    assert_eq!(
        turn_flow(&json!({"params":{"turn":{"status":"inProgress"}}})).unwrap(),
        ControlFlow::Continue(())
    );
    assert!(turn_flow(&json!({"params":{"turn":{"status":"failed"}}})).is_err());
    assert_eq!(
        error_flow(&json!({"params":{"willRetry":true}})).unwrap(),
        ControlFlow::Continue(())
    );
    assert!(error_flow(&json!({"params":{"willRetry":false}})).is_err());
}
