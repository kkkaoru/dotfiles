use std::collections::HashMap;

use serde_json::json;

use super::{ensure_background_batch_launch, external_tool::requested_external_tool_name};

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
