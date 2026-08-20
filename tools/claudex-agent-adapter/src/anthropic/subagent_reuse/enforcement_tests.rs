use serde_json::json;

use super::{
    extract_paths_from_text, extract_writer_paths, paths_overlap, should_reject_live_cap,
    should_reject_nested_launch,
};

#[test]
fn nested_sessions_reject_agent_launches_but_allow_send_message() {
    assert!(should_reject_nested_launch(
        true,
        &json!({"prompt":"keep editing src/lib.rs","claudex_model":"gpt-test"})
    ));
    assert!(!should_reject_nested_launch(
        false,
        &json!({"prompt":"keep editing src/lib.rs","claudex_model":"gpt-test"})
    ));
    assert!(!should_reject_nested_launch(
        true,
        &json!({"to":"a0123456789abcdef","message":"continue the review"})
    ));
}

#[test]
fn live_cap_rejects_new_agents_and_allows_follow_up() {
    assert!(should_reject_live_cap(
        2,
        2,
        &json!({"prompt":"edit src/main.rs","claudex_model":"gpt-test"})
    ));
    assert!(!should_reject_live_cap(
        1,
        2,
        &json!({"prompt":"edit src/main.rs","claudex_model":"gpt-test"})
    ));
    assert!(!should_reject_live_cap(
        2,
        2,
        &json!({"to":"a0123456789abcdef","message":"continue the review"})
    ));
    assert!(!should_reject_live_cap(
        1,
        0,
        &json!({"prompt":"edit src/main.rs","claudex_model":"gpt-test"})
    ));
}

#[test]
fn writer_paths_include_file_tokens_and_ignore_urls() {
    assert_eq!(
        extract_writer_paths(&json!({
            "cwd":"/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/tools/claudex-agent-adapter",
            "description":"Patch src/lib.rs",
            "prompt":"Edit `src/anthropic.rs` and ignore https://example.com/docs."
        })),
        vec!["src/lib.rs".to_owned(), "src/anthropic.rs".to_owned()]
    );
    assert_eq!(
        extract_paths_from_text("read ./src/main.rs and ../README.md"),
        vec!["./src/main.rs".to_owned(), "../readme.md".to_owned()]
    );
    assert_eq!(
        extract_paths_from_text("Nine Agents targeted capture.rs with different titles."),
        vec!["capture.rs".to_owned()]
    );
}

#[test]
fn overlapping_paths_include_nested_files() {
    assert!(paths_overlap(
        &["/tmp/adapter/src".to_owned()],
        &["/tmp/adapter/src/lib.rs".to_owned()]
    ));
    assert!(paths_overlap(
        &["src/lib.rs".to_owned()],
        &["src/lib.rs".to_owned()]
    ));
    assert!(!paths_overlap(
        &["src/lib.rs".to_owned()],
        &["src/main.rs".to_owned()]
    ));
    assert!(!paths_overlap(&["src/lib.rs".to_owned()], &[]));
}
