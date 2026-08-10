use std::time::Duration;

use tokio::time::timeout;

use super::*;

#[tokio::test]
async fn enforces_limit_and_reports_waiters() {
    let registry = ModelConcurrency::new(vec![("exact".to_owned(), 1)]);
    let first = registry
        .ticket("exact", Some(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let second = registry.ticket("exact", Some(1)).unwrap();
    let mut waiting = Box::pin(second.acquire_with_timeout(Duration::from_millis(100)));
    assert!(
        timeout(Duration::from_millis(10), waiting.as_mut())
            .await
            .is_err()
    );
    assert_eq!(
        registry.snapshot()["exact"],
        ModelConcurrencyStatus {
            active: 1,
            limit: 1,
            available: false,
            queued: 1,
        }
    );
    drop(first);
    let second = timeout(Duration::from_secs(1), waiting)
        .await
        .expect("waiting turn should acquire");
    drop(second.expect("released slot should acquire"));
    assert_eq!(registry.snapshot()["exact"].active, 0);
}

#[tokio::test]
async fn dynamic_exact_models_have_independent_limits() {
    let registry = ModelConcurrency::new(Vec::new());
    let first = registry
        .ticket("prefix-a", Some(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let second = timeout(
        Duration::from_millis(50),
        registry
            .ticket("prefix-b", Some(1))
            .unwrap()
            .acquire_with_timeout(Duration::from_millis(50)),
    )
    .await
    .expect("a different exact model must not share the permit");
    assert_eq!(registry.snapshot()["prefix-a"].active, 1);
    assert_eq!(registry.snapshot()["prefix-b"].active, 1);
    drop((first, second));
}

#[tokio::test]
async fn timeout_releases_queue_and_admission_permits() {
    let registry = ModelConcurrency::new(vec![("bounded".to_owned(), 1)]);
    let first = registry
        .ticket("bounded", Some(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let error = match registry
        .ticket("bounded", Some(1))
        .unwrap()
        .acquire_with_timeout(Duration::from_millis(1))
        .await
    {
        Ok(_) => panic!("occupied model should apply finite backpressure"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("model admission timed out"));
    assert_eq!(registry.snapshot()["bounded"].queued, 0);
    drop(first);
    let recovered = registry
        .ticket("bounded", Some(1))
        .unwrap()
        .acquire_with_timeout(Duration::from_millis(50))
        .await
        .expect("released model should admit a new turn");
    drop(recovered);
}

#[tokio::test]
async fn zero_wait_timeout_uses_nonblocking_admission() {
    let registry = ModelConcurrency::new(vec![("zero".to_owned(), 1)]);
    let first = registry
        .ticket("zero", Some(1))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let error = match registry
        .ticket("zero", Some(1))
        .unwrap()
        .acquire_with_timeout(Duration::ZERO)
        .await
    {
        Ok(_) => panic!("zero wait must not queue behind an occupied model"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("semaphore is unavailable"));
    drop(first);
}

#[tokio::test]
async fn interactive_zero_wait_falls_back_to_shared_slots() {
    let registry = ModelConcurrency::new(vec![("interactive-zero".to_owned(), 1)]);
    let first = registry
        .ticket("interactive-zero", Some(1))
        .unwrap()
        .acquire_with_timeout_for(Duration::from_millis(50), false)
        .await
        .unwrap();
    let error = match registry
        .ticket("interactive-zero", Some(1))
        .unwrap()
        .acquire_with_timeout_for(Duration::ZERO, true)
        .await
    {
        Ok(_) => panic!("interactive zero-wait must not block on a saturated model"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("semaphore is unavailable"),
        "{error:#}"
    );
    drop(first);
}

#[tokio::test]
async fn interactive_admission_times_out_when_both_pools_are_busy() {
    let registry = ModelConcurrency::new(vec![("interactive-timeout".to_owned(), 2)]);
    // Fill the shared slot first so the interactive acquire cannot steal it via
    // select!. Under llvm-cov load the reverse order flaked by taking slots.
    let shared = registry
        .ticket("interactive-timeout", Some(2))
        .unwrap()
        .acquire_with_timeout_for(Duration::from_secs(1), false)
        .await
        .unwrap();
    let interactive = registry
        .ticket("interactive-timeout", Some(2))
        .unwrap()
        .acquire_with_timeout_for(Duration::from_secs(1), true)
        .await
        .unwrap();
    let error = match registry
        .ticket("interactive-timeout", Some(2))
        .unwrap()
        .acquire_with_timeout_for(Duration::from_millis(1), true)
        .await
    {
        Ok(_) => panic!("busy interactive+shared pools must time out"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("model admission timed out"),
        "{error:#}"
    );
    drop((interactive, shared));
}

#[test]
fn parses_configured_wait_timeout_without_accepting_invalid_values() {
    assert_eq!(parse_wait_timeout(None), DEFAULT_WAIT_TIMEOUT);
    assert_eq!(parse_wait_timeout(Some("17")), Duration::from_millis(17));
    assert_eq!(parse_wait_timeout(Some("invalid")), DEFAULT_WAIT_TIMEOUT);
    assert_eq!(parse_wait_timeout(Some("-1")), DEFAULT_WAIT_TIMEOUT);
    assert_eq!(
        parse_wait_timeout(Some(&u64::MAX.to_string())),
        Duration::from_millis(u64::MAX)
    );
}

#[test]
fn reserves_a_finite_admission_window_per_model() {
    assert_eq!(admission_capacity(1), 3);
    assert_eq!(admission_capacity(4), 12);
    assert_eq!(admission_capacity(0), 0);
}

#[test]
fn detects_tui_qwen_concurrency_admission_timeout() {
    assert!(is_concurrency_admission_timeout(&anyhow!(
        "model `qwen3.8-max-preview` concurrency model admission timed out after 9.999999375s"
    )));
    assert!(is_concurrency_admission_timeout(&anyhow!(
        "model `qwen3.8-max-preview` concurrency model admission timed out after 29.999999375s"
    )));
    assert!(!is_concurrency_admission_timeout(&anyhow!(
        "Configured ACP completed with no assistant content"
    )));
}

#[test]
fn default_wait_timeout_is_ten_seconds() {
    assert_eq!(DEFAULT_WAIT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(parse_wait_timeout(None), Duration::from_secs(10));
}

#[tokio::test]
async fn subagent_capacity_is_slot_semaphore_not_full_limit() {
    let registry = ModelConcurrency::new(vec![("qwen3.8-max-preview".to_owned(), 3)]);
    let first = registry
        .ticket("qwen3.8-max-preview", Some(3))
        .unwrap()
        .acquire_for(false)
        .await
        .unwrap();
    let second = registry
        .ticket("qwen3.8-max-preview", Some(3))
        .unwrap()
        .acquire_for(false)
        .await
        .unwrap();
    assert!(registry.is_subagent_at_capacity("qwen3.8-max-preview"));
    assert!(
        registry.snapshot()["qwen3.8-max-preview"].available,
        "health snapshot still has the interactive reserve"
    );
    assert!(!registry.is_subagent_at_capacity("auto"));
    drop((first, second));
}
