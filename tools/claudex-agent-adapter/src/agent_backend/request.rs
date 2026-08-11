use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::Arc;

use crate::app_server::ThreadEvents;

use super::{BackendKind, RoutedBackend};

pub(super) async fn request_session(
    route: &Arc<RoutedBackend>,
    method: &str,
    params: Value,
) -> Result<Value> {
    if route.kind == BackendKind::CodexAppServer {
        let backend = route.get().await?;
        return Box::pin(backend.request(method, params)).await;
    }
    request_acp_session(route, method, params).await
}

async fn request_acp_session(
    route: &Arc<RoutedBackend>,
    method: &str,
    params: Value,
) -> Result<Value> {
    let backend = route.get().await?;
    let first = Box::pin(backend.request(method, params.clone())).await;
    let Err(error) = first else {
        return first;
    };
    if backend.is_alive() {
        return Err(error);
    }
    tracing::warn!(
        ?error,
        "restarting ACP provider after session creation failed"
    );
    route.retire();
    let restarted = route.get().await?;
    Box::pin(restarted.request(method, params))
        .await
        .with_context(|| format!("ACP session retry failed after initial error: {error:#}"))
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
