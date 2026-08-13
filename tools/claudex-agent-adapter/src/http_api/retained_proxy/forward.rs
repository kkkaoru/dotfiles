use axum::{
    Json,
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub(in crate::http_api) async fn proxy_request(
    client: &reqwest::Client,
    listen: std::net::SocketAddr,
    request: Request,
) -> Response {
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(request.uri().path());
    let url = format!("http://{listen}{path}");
    let mut upstream = client.request(request.method().clone(), url);
    for (name, value) in request.headers() {
        if is_hop_by_hop_header(name) {
            continue;
        }
        upstream = upstream.header(name, value);
    }
    let body = match axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": error.to_string()}})),
            )
                .into_response();
        }
    };
    match upstream.body(body).send().await {
        Ok(response) => map_upstream_response(response).await,
        Err(error) => crate::http_api::handover_circuit::retry_response(error.to_string()),
    }
}

async fn map_upstream_response(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers().clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(8);
    spawn_forward_response_chunks(response, tx);
    let mut mapped = Response::builder().status(status);
    for (name, value) in headers.iter() {
        mapped = mapped.header(name, value);
    }
    mapped
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn spawn_forward_response_chunks(
    response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>,
) {
    tokio::spawn(forward_response_chunks(response, tx));
}

async fn forward_response_chunks(
    mut response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>,
) {
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                let _ = tx.send(Err(std::io::Error::other(error))).await;
                break;
            }
        };
        if tx.send(Ok(chunk)).await.is_err() {
            break;
        }
    }
}

pub(in crate::http_api) fn is_hop_by_hop_header(name: &axum::http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}
