use super::*;
use super::handover::ServiceState;

#[test]
fn should_retry_idle_replace_respects_optional_limits() {
    assert!(should_retry_idle_replace(0, None));
    assert!(should_retry_idle_replace(9, None));
    assert!(should_retry_idle_replace(0, Some(0)));
    assert!(!should_retry_idle_replace(1, Some(0)));
    assert!(should_retry_idle_replace(2, Some(2)));
    assert!(!should_retry_idle_replace(3, Some(2)));
}

#[test]
fn listener_was_replaced_detects_replace_states() {
    assert!(listener_was_replaced(&ServiceState::Replace {
        pid: Some(1),
        recovery_generation: None,
    }));
    assert!(!listener_was_replaced(&ServiceState::Reuse));
    assert!(!listener_was_replaced(&ServiceState::Start));
    assert!(!listener_was_replaced(&ServiceState::Defer {
        pid: None,
        active_http_requests: 0,
        active_provider_turns: 0,
        active_subagents: 0,
    }));
}

#[test]
fn recovery_snapshot_is_missing_detects_not_found_io_errors() {
    let missing = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "missing snapshot",
    ));
    assert!(recovery_snapshot_is_missing(&missing));
    let nested = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "inner missing",
    ))
    .context("validate recovery generation");
    assert!(recovery_snapshot_is_missing(&nested));
    let other = anyhow::anyhow!("unrelated");
    assert!(!recovery_snapshot_is_missing(&other));
    let other_io = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "denied",
    ));
    assert!(!recovery_snapshot_is_missing(&other_io));
}
