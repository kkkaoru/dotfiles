use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
    queued: AtomicUsize,
}

pub(super) struct Ticket {
    entry: Arc<LimitedModel>,
}

pub(super) struct ModelPermit {
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
        Some(Ticket { entry })
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
    pub(super) async fn acquire(self) -> ModelPermit {
        self.entry.queued.fetch_add(1, Ordering::Relaxed);
        let queued = QueueGuard(&self.entry.queued);
        let permit = Arc::clone(&self.entry.slots)
            .acquire_owned()
            .await
            .expect("model concurrency semaphore is never closed");
        drop(queued);
        ModelPermit { _permit: permit }
    }
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
        let first = registry.ticket("exact", Some(1)).unwrap().acquire().await;
        let second = registry.ticket("exact", Some(1)).unwrap();
        let mut waiting = Box::pin(second.acquire());
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
        drop(second);
        assert_eq!(registry.snapshot()["exact"].active, 0);
    }

    #[tokio::test]
    async fn dynamic_exact_models_have_independent_limits() {
        let registry = ModelConcurrency::new(Vec::new());
        let first = registry
            .ticket("prefix-a", Some(1))
            .unwrap()
            .acquire()
            .await;
        let second = timeout(
            Duration::from_millis(50),
            registry.ticket("prefix-b", Some(1)).unwrap().acquire(),
        )
        .await
        .expect("a different exact model must not share the permit");
        assert_eq!(registry.snapshot()["prefix-a"].active, 1);
        assert_eq!(registry.snapshot()["prefix-b"].active, 1);
        drop((first, second));
    }
}
