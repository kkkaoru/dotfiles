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
/// Bound SubAgent admission waits so stacked workers fail over quickly instead
/// of freezing the TUI for half a minute. Override with
/// `CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS`.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

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
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "model_concurrency_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "model_concurrency_priority_tests.rs"]
mod priority_tests;
