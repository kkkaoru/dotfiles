use std::{
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::Instant,
};

use axum::{
    body::{Body, BodyDataStream, Bytes},
    extract::Request,
    extract::State,
    middleware::Next,
    response::Response,
};
use tokio_stream::Stream;

use super::health_route;

#[derive(Clone)]
pub(super) struct ActiveWorkState {
    pub(super) counter: Arc<AtomicUsize>,
    pub(super) last_work_at: Arc<Mutex<Instant>>,
}

pub(super) async fn track_active_http_request(
    State(work): State<ActiveWorkState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    work.counter.fetch_add(1, Ordering::Relaxed);
    let active = ActiveCounter::start(work.counter, work.last_work_at);
    hold_active_until_body_complete(next.run(request).await, active)
}

pub(super) async fn track_active_provider_turn(
    State(work): State<ActiveWorkState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    work.counter.fetch_add(1, Ordering::Relaxed);
    let active = ActiveCounter::start(work.counter, work.last_work_at);
    hold_active_until_body_complete(next.run(request).await, active)
}

fn hold_active_until_body_complete(
    response: Response<Body>,
    active: ActiveCounter,
) -> Response<Body> {
    response.map(|body| {
        Body::from_stream(ActiveBodyStream {
            inner: body.into_data_stream(),
            active: Some(active),
        })
    })
}

struct ActiveBodyStream {
    inner: BodyDataStream,
    active: Option<ActiveCounter>,
}

impl Stream for ActiveBodyStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let item = Pin::new(&mut self.inner).poll_next(cx);
        if matches!(&item, Poll::Ready(None) | Poll::Ready(Some(Err(_)))) {
            self.active.take();
        }
        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

struct ActiveCounter {
    counter: Arc<AtomicUsize>,
    last_work_at: Arc<Mutex<Instant>>,
}

impl ActiveCounter {
    fn start(counter: Arc<AtomicUsize>, last_work_at: Arc<Mutex<Instant>>) -> Self {
        health_route::touch_last_work(&last_work_at);
        Self {
            counter,
            last_work_at,
        }
    }
}

impl Drop for ActiveCounter {
    fn drop(&mut self) {
        // Mark completion so idle_seconds starts from the end of real work,
        // not from process start or the last coincidental /health probe.
        health_route::touch_last_work(&self.last_work_at);
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}
