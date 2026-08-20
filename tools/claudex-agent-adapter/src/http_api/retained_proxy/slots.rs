use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::oneshot;

pub(in crate::http_api) const MAX_PROXY_IN_FLIGHT: usize = 256;

struct InFlight {
    id: u64,
    cancel: oneshot::Sender<()>,
}

struct Inner {
    limit: usize,
    next_id: AtomicU64,
    inflight: Mutex<VecDeque<InFlight>>,
}

pub(in crate::http_api) struct ProxySlotPool {
    inner: Arc<Inner>,
}

pub(in crate::http_api) struct ProxySlot {
    id: u64,
    abort: oneshot::Receiver<()>,
    inner: Arc<Inner>,
}

impl Inner {
    fn lock_inflight(&self) -> MutexGuard<'_, VecDeque<InFlight>> {
        self.inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProxySlotPool {
    pub(in crate::http_api) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                limit: limit.max(1),
                next_id: AtomicU64::new(1),
                inflight: Mutex::new(VecDeque::new()),
            }),
        })
    }

    pub(in crate::http_api) fn acquire(&self) -> ProxySlot {
        let (cancel, abort) = oneshot::channel();
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut inflight = self.inner.lock_inflight();
        while inflight.len() >= self.inner.limit {
            Self::evict_oldest(&mut inflight);
        }
        inflight.push_back(InFlight { id, cancel });
        ProxySlot {
            id,
            abort,
            inner: Arc::clone(&self.inner),
        }
    }

    fn evict_oldest(inflight: &mut VecDeque<InFlight>) {
        let Some(oldest) = inflight.pop_front() else {
            return;
        };
        let _ = oldest.cancel.send(());
    }
}

impl ProxySlot {
    pub(in crate::http_api) async fn evicted(&mut self) {
        let _ = (&mut self.abort).await;
    }
}

impl Drop for ProxySlot {
    fn drop(&mut self) {
        self.inner
            .lock_inflight()
            .retain(|entry| entry.id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn max_proxy_in_flight_is_two_hundred_fifty_six() {
        assert_eq!(MAX_PROXY_IN_FLIGHT, 256);
    }

    #[tokio::test]
    async fn acquire_evicts_the_oldest_slot_when_full() {
        let pool = ProxySlotPool::new(2);
        let mut first = pool.acquire();
        let mut second = pool.acquire();
        let mut third = pool.acquire();
        first.evicted().await;
        let second_still_held =
            tokio::time::timeout(Duration::from_millis(20), second.evicted()).await;
        assert!(second_still_held.is_err());
        let third_still_held =
            tokio::time::timeout(Duration::from_millis(20), third.evicted()).await;
        assert!(third_still_held.is_err());
    }

    #[tokio::test]
    async fn acquire_does_not_evict_when_under_the_limit() {
        let pool = ProxySlotPool::new(2);
        let mut first = pool.acquire();
        let mut second = pool.acquire();
        let first_still_held =
            tokio::time::timeout(Duration::from_millis(20), first.evicted()).await;
        let second_still_held =
            tokio::time::timeout(Duration::from_millis(20), second.evicted()).await;
        assert!(first_still_held.is_err());
        assert!(second_still_held.is_err());
    }

    #[tokio::test]
    async fn dropping_a_slot_frees_capacity_without_evicting_the_remaining() {
        let pool = ProxySlotPool::new(1);
        let first = pool.acquire();
        drop(first);
        let mut second = pool.acquire();
        let second_still_held =
            tokio::time::timeout(Duration::from_millis(20), second.evicted()).await;
        assert!(second_still_held.is_err());
    }

    #[tokio::test]
    async fn a_zero_limit_still_holds_one_slot_and_evicts_the_oldest() {
        let pool = ProxySlotPool::new(0);
        let mut first = pool.acquire();
        let mut second = pool.acquire();
        first.evicted().await;
        let second_still_held =
            tokio::time::timeout(Duration::from_millis(20), second.evicted()).await;
        assert!(second_still_held.is_err());
    }
}
