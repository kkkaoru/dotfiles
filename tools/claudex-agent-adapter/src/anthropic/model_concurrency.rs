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
    admission: Arc<Semaphore>,
    queued: AtomicUsize,
}

pub(super) struct Ticket {
    entry: Arc<LimitedModel>,
    model: String,
}

pub(super) struct ModelPermit {
    _admission: OwnedSemaphorePermit,
    _permit: OwnedSemaphorePermit,
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
            slots: Arc::new(Semaphore::new(limit)),
            admission: Arc::new(Semaphore::new(admission_capacity(limit))),
            queued: AtomicUsize::new(0),
        }
    }

    fn status(&self) -> ModelConcurrencyStatus {
        let free_permits = self.slots.available_permits();
        let queued = self.queued.load(Ordering::Relaxed);
        ModelConcurrencyStatus {
            active: self.limit - free_permits,
            limit: self.limit,
            available: self.limit - free_permits + queued < self.limit,
            queued,
        }
    }
}

impl Ticket {
    pub(super) async fn acquire(self) -> Result<ModelPermit> {
        self.acquire_with_timeout(model_concurrency_wait_timeout())
            .await
    }

    async fn acquire_with_timeout(self, wait_timeout: Duration) -> Result<ModelPermit> {
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
        let permit = acquire_permit(
            Arc::clone(&self.entry.slots),
            remaining,
            "model",
            &self.model,
        )
        .await?;
        drop(queued);
        Ok(ModelPermit {
            _admission: admission,
            _permit: permit,
        })
    }
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
