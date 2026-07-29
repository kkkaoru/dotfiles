use std::{collections::HashSet, sync::Arc};

use super::{AgentBackend, RoutedBackend, RoutedBackends, StartupState};

impl RoutedBackend {
    fn take_ready_backend_for_shutdown(&self) -> Option<Arc<AgentBackend>> {
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
