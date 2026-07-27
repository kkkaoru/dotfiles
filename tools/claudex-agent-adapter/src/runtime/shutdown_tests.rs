use std::{sync::Arc, time::Duration};

use axum::{extract::State, routing::get};
use tokio::sync::Notify;

const LISTENER_RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
const LISTENER_RETRY_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone)]
struct SlowResponse {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

async fn slow_response(State(state): State<SlowResponse>) -> &'static str {
    state.entered.notify_one();
    state.release.notified().await;
    "complete"
}

#[tokio::test]
async fn drains_an_active_response_before_server_shutdown() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let router = Router::new()
        .route("/slow", get(slow_response))
        .with_state(SlowResponse {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("listener address");
    let shutdown = Arc::new(Notify::new());
    let server = tokio::spawn(serve_until(listener, router, {
        let shutdown = Arc::clone(&shutdown);
        async move { shutdown.notified().await }
    }));
    let response = tokio::spawn(async move {
        reqwest::get(format!("http://{address}/slow"))
            .await
            .expect("response")
            .text()
            .await
            .expect("response body")
    });
    entered.notified().await;
    shutdown.notify_one();
    let replacement_listener = tokio::time::timeout(LISTENER_RELEASE_TIMEOUT, async {
        loop {
            match tokio::net::TcpListener::bind(address).await {
                Ok(listener) => break listener,
                Err(_) => tokio::time::sleep(LISTENER_RETRY_INTERVAL).await,
            }
        }
    })
    .await
    .expect("graceful shutdown must release the listener before responses drain");
    assert!(!response.is_finished());
    drop(replacement_listener);
    release.notify_one();
    assert_eq!(response.await.expect("request task"), "complete");
    server
        .await
        .expect("server task")
        .expect("graceful server shutdown");
}
