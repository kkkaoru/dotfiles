use std::{net::SocketAddr, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::super::retained_health::RetainedHealthProbe;

pub(in crate::http_api) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(400);

pub(in crate::http_api) enum ListenHealth {
    Ready(RetainedHealthProbe),
    /// Connect refused or reachable `/health` with status != "ok".
    Unreachable,
    /// Timeout or 2xx that is not parseable `{"status":"ok"}`.
    Transient,
}

pub(in crate::http_api) enum ProxyOutcome {
    Response(Response),
    TransportFailed(String),
}

impl ProxyOutcome {
    pub(in crate::http_api) fn into_response(self) -> Response {
        match self {
            Self::Response(response) => response,
            Self::TransportFailed(message) => {
                crate::http_api::handover_circuit::retry_response(message)
            }
        }
    }
}

/// Live listen: HTTP 2xx `/health` within 400ms and parseable `status=="ok"`.
/// 2xx HTML/garbage is Transient (not Ready). Connect fail is Unreachable.
pub(in crate::http_api) async fn probe_listen_health(
    client: &reqwest::Client,
    listen: SocketAddr,
) -> ListenHealth {
    let response = match client
        .get(format!("http://{listen}/health"))
        .timeout(HEALTH_PROBE_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return ListenHealth::Transient,
        Err(error) if error.is_connect() => return ListenHealth::Unreachable,
        Err(_) => return ListenHealth::Transient,
    };
    if !response.status().is_success() {
        return ListenHealth::Transient;
    }
    match response.json::<RetainedHealthProbe>().await {
        Ok(health) if health.status == "ok" => ListenHealth::Ready(health),
        Ok(_) => ListenHealth::Unreachable,
        Err(_) => ListenHealth::Transient,
    }
}

/// True only for a Ready listen. Never proxy on Transient or Unreachable.
pub(in crate::http_api) async fn listen_accepts_health(
    client: &reqwest::Client,
    listen: SocketAddr,
) -> bool {
    matches!(
        probe_listen_health(client, listen).await,
        ListenHealth::Ready(_)
    )
}

pub(in crate::http_api) async fn proxy_request(
    client: &reqwest::Client,
    listen: std::net::SocketAddr,
    request: Request,
) -> ProxyOutcome {
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
            return ProxyOutcome::Response(
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": {"message": error.to_string()}})),
                )
                    .into_response(),
            );
        }
    };
    match upstream.body(body).send().await {
        Ok(response) => ProxyOutcome::Response(map_upstream_response(response).await),
        Err(error) => ProxyOutcome::TransportFailed(error.to_string()),
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
