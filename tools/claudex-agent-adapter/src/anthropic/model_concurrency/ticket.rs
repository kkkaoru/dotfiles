use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::acquire::{acquire_interactive_permit, acquire_permit};
use super::{ModelPermit, QueueGuard, Ticket, model_concurrency_wait_timeout};

impl Ticket {
    #[cfg(test)]
    pub(in crate::anthropic) async fn acquire(self) -> Result<ModelPermit> {
        self.acquire_with_timeout(model_concurrency_wait_timeout())
            .await
    }

    pub(in crate::anthropic) async fn acquire_for(self, interactive: bool) -> Result<ModelPermit> {
        self.acquire_with_timeout_for(model_concurrency_wait_timeout(), interactive)
            .await
    }

    #[cfg(test)]
    pub(super) async fn acquire_with_timeout(self, wait_timeout: Duration) -> Result<ModelPermit> {
        self.acquire_with_timeout_for(wait_timeout, false).await
    }

    pub(super) async fn acquire_with_timeout_for(
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


