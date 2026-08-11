use std::collections::HashMap;

use serde_json::json;

use super::{
    batch::ensure_background_batch_launch,
    external_tool::{requested_external_tool_name, unrequested_tool_reply},
};

#[test]
fn leaves_non_object_batch_arguments_unchanged() {
    let mut arguments = json!(null);
    ensure_background_batch_launch(&mut arguments);
    assert_eq!(arguments, json!(null));
}

#[test]
fn accepts_only_dynamic_or_original_names_of_supplied_external_tools() {
    let names = HashMap::from([("cc_lookup_0".to_owned(), "lookup".to_owned())]);

    assert_eq!(
        requested_external_tool_name(&names, "cc_lookup_0"),
        Some("lookup")
    );
    assert_eq!(
        requested_external_tool_name(&names, "lookup"),
        Some("lookup")
    );
    assert_eq!(requested_external_tool_name(&names, "unrequested"), None);
}

#[test]
fn invented_advisor_calls_continue_without_a_hard_tool_error() {
    let (text, success) = unrequested_tool_reply("advisor");
    assert!(
        success,
        "retrying advisor() after No such tool available is the old failure"
    );
    assert!(text.contains("main-session only"));
    assert!(text.contains("disabled_subagent_models"));
    let (cc_text, cc_success) = unrequested_tool_reply("cc_advisor_0");
    assert!(cc_success);
    assert!(cc_text.contains("main-session only"));
    let (other_text, other_success) = unrequested_tool_reply("unrequested");
    assert!(!other_success);
    assert!(other_text.contains("unrequested"));
}
