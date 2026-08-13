use std::hint::black_box;

use super::detect::{
    count_explicit_blocks, is_explicit_block, is_numbered_block, is_remaining_only_follow_up,
};

#[test]
fn remaining_follow_up_rejects_an_explicit_worker_count() {
    assert!(!is_remaining_only_follow_up(black_box(
        "remaining work and 5 workers"
    )));
    assert!(is_remaining_only_follow_up(black_box("remaining work")));
}

#[test]
fn explicit_blocks_cover_bullet_and_empty_numbered_edges() {
    assert!(is_explicit_block(black_box("- one")));
    assert!(is_explicit_block(black_box("* two")));
    assert!(is_explicit_block(black_box("・three")));
    assert_eq!(
        count_explicit_blocks(black_box("- one\n* two\n・three\n4. four")),
        4
    );
    assert!(!is_numbered_block(black_box("")));
    assert!(!is_numbered_block(black_box("plain")));
    assert!(!is_explicit_block(black_box("")));
}
