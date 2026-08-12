use std::{collections::HashSet, sync::Arc};

use super::{AgentBackend, RoutedBackend, RoutedBackends, StartupState};

impl RoutedBackend {
    fn take_ready_backend_for_shutdown(&self) -> Option<Arc<AgentBackend>> {
        self.startup
            .closed
            .store(true, std::sync::atomic::Ordering::Release);
        // A caller may still be waiting on a cloned Starting receiver while
        // shutdown removes the canonical receiver. Fence that caller before
        // dropping the route so a late spawn result cannot be republished.
        self.startup
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let receiver = self
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned")
            .take()?;
        match receiver.borrow().clone() {
            StartupState::Ready(Ok(backend)) => Some(backend),
            StartupState::Starting | StartupState::Ready(Err(_)) => None,
        }
    }
}

impl RoutedBackends {
    pub(crate) async fn shutdown(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
        let dynamic = self
            .dynamic
            .lock()
            .expect("dynamic routes poisoned")
            .clone();
        let routes = self
            .configured
            .iter()
            .cloned()
            .chain(dynamic)
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        for backend in routes
            .iter()
            .filter_map(|route| route.take_ready_backend_for_shutdown())
            .filter(|backend| seen.insert(Arc::as_ptr(backend) as usize))
        {
            backend.shutdown_leaf().await;
        }
    }
}
