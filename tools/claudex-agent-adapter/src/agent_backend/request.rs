use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

use crate::app_server::ThreadEvents;

use super::RoutedBackend;

pub(super) async fn request_session(
    route: &Arc<RoutedBackend>,
    method: &str,
    params: Value,
) -> Result<Value> {
    let backend = route.get().await?;
    Box::pin(backend.request(method, params)).await
}

pub(super) fn routed_thread(thread_id: &str) -> (usize, &str) {
    let (index, raw_id) = thread_id
        .split_once(':')
        .expect("routed backend thread ID is missing its route prefix");
    (index.parse().expect("invalid routed backend index"), raw_id)
}

pub(super) fn subscribe_routed_thread(
    route: &RoutedBackend,
    thread_id: &str,
    raw_id: &str,
) -> ThreadEvents {
    match route.ready_backend() {
        Some(backend) => backend.subscribe_thread(raw_id),
        None => {
            tracing::warn!(
                thread_id,
                model = %route.model,
                "thread route backend was retired before event subscription"
            );
            ThreadEvents::closed(raw_id)
        }
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod request_tests;
