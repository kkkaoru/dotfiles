use std::hint::black_box;

use super::actions::{count_for_content, is_action_item, is_indented_continuation};

#[test]
fn single_action_items_do_not_count_as_parallel_scopes() {
    assert_eq!(
        count_for_content(black_box(
            "Please:\n- Implement only the parser\n\nNotes:\n  continued"
        )),
        1
    );
    assert_eq!(
        count_for_content(black_box(
            "Tasks:\n- do not leak secrets\n- Implement parser"
        )),
        1
    );
}

#[test]
fn action_items_reject_negated_and_diagnostic_bodies() {
    assert!(!is_action_item(black_box("do not leak secrets")));
    assert!(!is_action_item(black_box("error: worker launch failed")));
    assert!(is_action_item(black_box("Implement parser")));
}

#[test]
fn indented_continuation_rejects_blank_and_flush_lines() {
    assert!(!is_indented_continuation(black_box("")));
    assert!(!is_indented_continuation(black_box("   ")));
    assert!(!is_indented_continuation(black_box("flush")));
    assert!(is_indented_continuation(black_box("    continued")));
}
