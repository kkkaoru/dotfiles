use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
use anyhow::anyhow;
use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

mod acquire;
#[cfg(test)]
use acquire::parse_wait_timeout;
use acquire::{admission_capacity, model_concurrency_wait_timeout};

pub(super) const MODEL_CONCURRENCY_WAIT_TIMEOUT_ENV: &str =
    "CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS";
/// Bound SubAgent admission waits so stacked workers fail over quickly instead
/// of freezing the TUI for half a minute. Override with
/// `CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS`.
pub(super) const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

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

    /// SubAgent turns only take the non-interactive slot semaphore
    /// (`limit - 1`). Health `available` still compares active+queued against
    /// the full limit, so two Qwen SubAgents can look free while the next
    /// SubAgent waits 30s and surfaces the TUI admission timeout.
    pub(super) fn is_subagent_at_capacity(&self, model: &str) -> bool {
        self.entries
            .lock()
            .expect("model concurrency registry poisoned")
            .get(model)
            .is_some_and(|entry| entry.slots.available_permits() == 0)
    }
}

pub(super) fn is_concurrency_admission_timeout(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("concurrency") && message.contains("admission timed out")
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

mod ticket;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "model_concurrency_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "model_concurrency_priority_tests.rs"]
mod priority_tests;
