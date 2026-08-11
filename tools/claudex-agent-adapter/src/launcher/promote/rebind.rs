use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    time::Instant,
};

use anyhow::{Result, bail};
use serde_json::json;

use super::{HANDOVER_POLL, HANDOVER_TIMEOUT, RebindResponse};
use super::super::ServiceConfig;

pub(super) async fn restore_old_canonical(
    client: &reqwest::Client,
    config: &ServiceConfig,
    retained_listen: SocketAddr,
) {
    let _ = request_bind_listen(
        client,
        &config.with_listen(retained_listen),
        config.options.listen,
    )
    .await;
}

pub(super) async fn request_ephemeral_rebind(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<Option<RebindResponse>> {
    request_rebind(client, config, json!({ "ephemeral": true })).await
}

pub(super) async fn request_bind_listen(
    client: &reqwest::Client,
    target: &ServiceConfig,
    listen: SocketAddr,
) -> Result<Option<RebindResponse>> {
    request_rebind(client, target, json!({ "listen": listen.to_string() })).await
}

async fn request_rebind(
    client: &reqwest::Client,
    target: &ServiceConfig,
    body: serde_json::Value,
) -> Result<Option<RebindResponse>> {
    let response = match client
        .post(format!("{}/admin/rebind-listener", target.base_url()))
        .bearer_auth(&target.token)
        .json(&body)
        .timeout(HANDOVER_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    Ok(response.json().await.ok())
}

pub(super) fn listen_is_free(listen: SocketAddr) -> bool {
    StdTcpListener::bind(listen).is_ok()
}

pub(super) async fn wait_until_canonical_released(config: &ServiceConfig) -> Result<()> {
    let deadline = Instant::now() + HANDOVER_TIMEOUT;
    loop {
        if listen_is_free(config.options.listen) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "canonical listener {} did not release after handover",
                config.options.listen
            );
        }
        tokio::time::sleep(HANDOVER_POLL).await;
    }
}
