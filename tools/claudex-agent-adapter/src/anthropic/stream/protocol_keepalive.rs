use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    http::{Response, StatusCode, header},
};
use tokio::{
    sync::mpsc,
    time::{Instant, Sleep, sleep},
};
use tokio_stream::Stream;

use super::SSE_KEEPALIVE_FRAME;

pub(super) fn streaming_sse_response_with_interval(
    receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
    interval: Duration,
) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(KeepaliveStream::new(receiver, interval)))
        .expect("valid streaming response")
}

pub(super) struct KeepaliveStream {
    receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
    interval: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl KeepaliveStream {
    pub(super) fn new(receiver: mpsc::Receiver<Result<Bytes, Infallible>>, interval: Duration) -> Self {
        Self {
            receiver,
            interval,
            deadline: Box::pin(sleep(interval)),
        }
    }

    fn reset_deadline(&mut self) {
        self.deadline.as_mut().reset(Instant::now() + self.interval);
    }
}

impl Stream for KeepaliveStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        match stream.receiver.poll_recv(context) {
            Poll::Ready(Some(frame)) => {
                stream.reset_deadline();
                return Poll::Ready(Some(frame));
            }
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        if stream.deadline.as_mut().poll(context).is_ready() {
            stream.reset_deadline();
            return Poll::Ready(Some(Ok(Bytes::from_static(SSE_KEEPALIVE_FRAME))));
        }
        Poll::Pending
    }
}

