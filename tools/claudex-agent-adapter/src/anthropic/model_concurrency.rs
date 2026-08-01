use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use serde::Serialize;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, timeout},
};

pub(super) const MODEL_CONCURRENCY_WAIT_TIMEOUT_ENV: &str =
    "CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS";
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ModelConcurrencyStatus {
    active: usize,
    limit: usize,
    available: bool,
    queued: usize,
}

pub(super) struct ModelConcurrency {
    entries: Mutex<HashMap<String, Arc<LimitedModel>>>,
}

struct LimitedModel {
    limit: usize,
    slots: Arc<Semaphore>,
    interactive: Arc<Semaphore>,
    admission: Arc<Semaphore>,
    queued: AtomicUsize,
    active: AtomicUsize,
}

pub(super) struct Ticket {
    entry: Arc<LimitedModel>,
    model: String,
}

pub(super) struct ModelPermit {
    _admission: OwnedSemaphorePermit,
    _permit: OwnedSemaphorePermit,
    entry: Arc<LimitedModel>,
}

impl Drop for ModelPermit {
    fn drop(&mut self) {
        self.entry.active.fetch_sub(1, Ordering::Relaxed);
    }
}

struct QueueGuard<'a>(&'a AtomicUsize);

impl Drop for QueueGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ModelConcurrency {
    pub(super) fn new(limits: Vec<(String, usize)>) -> Self {
        let entries = limits
            .into_iter()
            .map(|(model, limit)| (model, Arc::new(LimitedModel::new(limit))))
            .collect();
        Self {
            entries: Mutex::new(entries),
        }
    }

    pub(super) fn ticket(&self, model: &str, limit: Option<usize>) -> Option<Ticket> {
        let limit = limit?;
        let entry = Arc::clone(
            self.entries
                .lock()
                .expect("model concurrency registry poisoned")
                .entry(model.to_owned())
                .or_insert_with(|| Arc::new(LimitedModel::new(limit))),
        );
        debug_assert_eq!(entry.limit, limit, "model concurrency limit changed");
        Some(Ticket {
            entry,
            model: model.to_owned(),
        })
    }

    pub(super) fn snapshot(&self) -> BTreeMap<String, ModelConcurrencyStatus> {
        self.entries
            .lock()
            .expect("model concurrency registry poisoned")
            .iter()
            .map(|(model, entry)| (model.clone(), entry.status()))
            .collect()
    }
}

impl LimitedModel {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            slots: Arc::new(Semaphore::new(limit.saturating_sub(1).max(1))),
            interactive: Arc::new(Semaphore::new(usize::from(limit > 1))),
            admission: Arc::new(Semaphore::new(admission_capacity(limit))),
            queued: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
        }
    }

    fn status(&self) -> ModelConcurrencyStatus {
        let active = self.active.load(Ordering::Relaxed);
        let queued = self.queued.load(Ordering::Relaxed);
        ModelConcurrencyStatus {
            active,
            limit: self.limit,
            available: active + queued < self.limit,
            queued,
        }
    }
}

impl Ticket {
    #[cfg(test)]
    pub(super) async fn acquire(self) -> Result<ModelPermit> {
        self.acquire_with_timeout(model_concurrency_wait_timeout())
            .await
    }

    pub(super) async fn acquire_for(self, interactive: bool) -> Result<ModelPermit> {
        self.acquire_with_timeout_for(model_concurrency_wait_timeout(), interactive)
            .await
    }

    #[cfg(test)]
    async fn acquire_with_timeout(self, wait_timeout: Duration) -> Result<ModelPermit> {
        self.acquire_with_timeout_for(wait_timeout, false).await
    }

    async fn acquire_with_timeout_for(
        self,
        wait_timeout: Duration,
        interactive: bool,
    ) -> Result<ModelPermit> {
        let started = Instant::now();
        let admission = acquire_permit(
            Arc::clone(&self.entry.admission),
            wait_timeout,
            "admission",
            &self.model,
        )
        .await?;
        self.entry.queued.fetch_add(1, Ordering::Relaxed);
        let queued = QueueGuard(&self.entry.queued);
        let remaining = wait_timeout.saturating_sub(started.elapsed());
        let permit = if interactive {
            acquire_interactive_permit(&self.entry, remaining, &self.model).await?
        } else {
            acquire_permit(
                Arc::clone(&self.entry.slots),
                remaining,
                "model",
                &self.model,
            )
            .await?
        };
        drop(queued);
        self.entry.active.fetch_add(1, Ordering::Relaxed);
        Ok(ModelPermit {
            _admission: admission,
            _permit: permit,
            entry: Arc::clone(&self.entry),
        })
    }
}

async fn acquire_interactive_permit(
    entry: &LimitedModel,
    wait_timeout: Duration,
    model: &str,
) -> Result<OwnedSemaphorePermit> {
    if wait_timeout.is_zero() {
        return entry
            .interactive
            .clone()
            .try_acquire_owned()
            .or_else(|_| entry.slots.clone().try_acquire_owned())
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"));
    }
    timeout(wait_timeout, async {
        tokio::select! {
            permit = entry.interactive.clone().acquire_owned() => permit,
            permit = entry.slots.clone().acquire_owned() => permit,
        }
    })
    .await
    .map_err(|_| {
        anyhow!("model `{model}` concurrency model admission timed out after {wait_timeout:?}")
    })?
    .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
}

async fn acquire_permit(
    semaphore: Arc<Semaphore>,
    wait_timeout: Duration,
    stage: &str,
    model: &str,
) -> Result<OwnedSemaphorePermit> {
    if wait_timeout.is_zero() {
        semaphore
            .try_acquire_owned()
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
    } else {
        timeout(wait_timeout, semaphore.acquire_owned())
            .await
            .map_err(|_| {
                anyhow!(
                    "model `{model}` concurrency {stage} admission timed out after {wait_timeout:?}"
                )
            })?
            .map_err(|_| anyhow!("model `{model}` concurrency semaphore is unavailable"))
    }
}

fn admission_capacity(limit: usize) -> usize {
    limit
        .saturating_mul(3)
        .min(Semaphore::MAX_PERMITS)
        .max(limit)
}

fn model_concurrency_wait_timeout() -> Duration {
    parse_wait_timeout(
        std::env::var(MODEL_CONCURRENCY_WAIT_TIMEOUT_ENV)
            .ok()
            .as_deref(),
    )
}

fn parse_wait_timeout(value: Option<&str>) -> Duration {
    let Some(value) = value else {
        return DEFAULT_WAIT_TIMEOUT;
    };
    let Ok(milliseconds) = value.parse::<u64>() else {
        return DEFAULT_WAIT_TIMEOUT;
    };
    Duration::from_millis(milliseconds)
}

#[cfg(test)]
// Coverage gates measure production concurrency; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
}

#[cfg(test)]
#[path = "model_concurrency_priority_tests.rs"]
mod priority_tests;
