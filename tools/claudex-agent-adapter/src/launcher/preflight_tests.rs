use super::*;

fn test_config() -> ServiceConfig {
    ServiceConfig::new(super::super::AdapterOptions {
        routes: Vec::new(),
        model: "test-model".to_owned(),
        listen: "127.0.0.1:0".parse().unwrap(),
        subscription_max_processes: 1,
        subscription_timeout_minutes: 1,
        subagent_hard_timeout_seconds: None,
        model_catalog: crate::provider_config::ModelCatalog::default(),
    })
    .unwrap()
}

fn ready_ok<'a>(
    _: &'a reqwest::Client,
    _: &'a ServiceConfig,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(std::future::ready(Ok(())))
}

fn ready_err<'a>(
    _: &'a reqwest::Client,
    _: &'a ServiceConfig,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(std::future::ready(Err(anyhow::anyhow!("not ready"))))
}

fn release_ok<'a>(
    _: &'a reqwest::Client,
    _: &'a ServiceConfig,
    _: u32,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(std::future::ready(Ok(())))
}

fn release_err<'a>(
    _: &'a reqwest::Client,
    _: &'a ServiceConfig,
    _: u32,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(std::future::ready(Err(anyhow::anyhow!("release failed"))))
}

#[test]
fn reserves_matching_address_families_for_preflight() {
    assert!(
        isolated_listen("0.0.0.0:8318".parse().unwrap())
            .unwrap()
            .is_ipv4()
    );
    assert!(
        isolated_listen("[::]:8318".parse().unwrap())
            .unwrap()
            .is_ipv6()
    );
}

#[test]
fn force_cleans_a_preflight_that_refuses_graceful_shutdown() {
    let terminated = std::sync::atomic::AtomicU32::new(0);
    let result = finish_preflight_shutdown(77, Err(anyhow::Error::msg("deadline")), |pid| {
        terminated.store(pid, std::sync::atomic::Ordering::Relaxed);
    });
    assert!(result.is_err());
    assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 77);
}

#[test]
fn successful_preflight_release_does_not_force_terminate() {
    let result = finish_preflight_shutdown(81, Ok(()), |_| {
        panic!("successful release must not force terminate")
    });
    assert!(result.is_ok());
}

#[tokio::test]
async fn verify_reports_when_isolated_preflight_cannot_start() {
    let config = test_config();
    let result = verify_with_hooks(
        &reqwest::Client::new(),
        &config,
        |_| Err(anyhow::anyhow!("cannot bind dummy")),
        ready_ok,
        release_ok,
        |_, _| true,
        |_| panic!("start failure must not terminate"),
    )
    .await;
    let error = result.expect_err("isolated start failure");
    assert!(
        error
            .to_string()
            .contains("start isolated adapter preflight"),
        "{error:#}"
    );
}

#[tokio::test]
async fn verify_runs_the_full_successful_preflight_lifecycle() {
    let config = test_config();
    let result = verify_with_hooks(
        &reqwest::Client::new(),
        &config,
        |_| Ok(77),
        ready_ok,
        release_ok,
        |_, _| true,
        |_| panic!("successful preflight does not force terminate"),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn verify_terminates_a_matching_preflight_when_readiness_fails() {
    let config = test_config();
    let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let observed = std::sync::Arc::clone(&terminated);
    let result = verify_with_hooks(
        &reqwest::Client::new(),
        &config,
        |_| Ok(78),
        ready_err,
        release_ok,
        |_, _| true,
        move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 78);
}

#[tokio::test]
async fn verify_keeps_an_unmatched_process_when_readiness_fails() {
    let config = test_config();
    let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let observed = std::sync::Arc::clone(&terminated);
    let result = verify_with_hooks(
        &reqwest::Client::new(),
        &config,
        |_| Ok(79),
        ready_err,
        release_ok,
        |_, _| false,
        move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn verify_force_terminates_when_graceful_release_fails() {
    let config = test_config();
    let terminated = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let observed = std::sync::Arc::clone(&terminated);
    let result = verify_with_hooks(
        &reqwest::Client::new(),
        &config,
        |_| Ok(80),
        ready_ok,
        release_err,
        |_, _| true,
        move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(terminated.load(std::sync::atomic::Ordering::Relaxed), 80);
}
