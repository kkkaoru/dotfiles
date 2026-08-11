use std::{
    env,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::AcpProvider;
use super::super::client::AcpClient;
use crate::app_server::events::ThreadEventDispatcher;

pub(super) fn wire_provider_connection(
    provider: AcpProvider,
    events: Arc<ThreadEventDispatcher>,
    child: &mut tokio::process::Child,
    alive: Arc<AtomicBool>,
) -> Result<(acp::ClientSideConnection, oneshot::Receiver<()>)> {
    let outgoing = child
        .stdin
        .take()
        .with_context(|| format!("{} ACP stdin is unavailable", provider.label()))?
        .compat_write();
    let incoming = child
        .stdout
        .take()
        .with_context(|| format!("{} ACP stdout is unavailable", provider.label()))?
        .compat();
    let client = AcpClient::new(events);
    let (connection, handle_io) =
        acp::ClientSideConnection::new(client, outgoing, incoming, |future| {
            tokio::task::spawn_local(future);
        });
    let (io_stopped, io_stopped_rx) = oneshot::channel();
    let provider_label = provider.label();
    tokio::task::spawn_local(async move {
        if let Err(error) = handle_io.await {
            tracing::error!(
                ?error,
                provider = provider_label,
                "ACP I/O stopped (provider likely exited; recycle the route)"
            );
        }
        mark_io_stopped(&alive, io_stopped);
    });
    Ok((connection, io_stopped_rx))
}

pub(super) fn mark_io_stopped(alive: &AtomicBool, io_stopped: oneshot::Sender<()>) {
    alive.store(false, Ordering::Relaxed);
    let _ = io_stopped.send(());
}

pub(super) async fn initialize(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
) -> Result<()> {
    let response = connection
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_info(acp::Implementation::new(
                    "claudex-agent-adapter",
                    env!("CARGO_PKG_VERSION"),
                ))
                .meta(
                    json!({
                        "startupHints": {
                            "nonInteractive": true,
                            "skipGitStatus": true,
                            "skipProjectLayout": true
                        },
                        "clientType":"claudex-agent-adapter"
                    })
                    .as_object()
                    .cloned(),
                ),
        )
        .await
        .map_err(|error| anyhow!("{} ACP initialize failed: {error:?}", provider.label()))?;
    if response.protocol_version != acp::ProtocolVersion::V1 {
        bail!(
            "{} ACP selected unsupported protocol version",
            provider.label()
        )
    }
    let preferred = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("defaultAuthMethodId"))
        .and_then(Value::as_str);
    let method = preferred
        .and_then(|id| {
            response
                .auth_methods
                .iter()
                .find(|method| method.id().0.as_ref() == id)
        })
        .or_else(|| response.auth_methods.first());
    if let Some(method) = method {
        connection
            .authenticate(
                acp::AuthenticateRequest::new(method.id().clone())
                    .meta(json!({"headless":true}).as_object().cloned()),
            )
            .await
            .map_err(|error| {
                anyhow!("{} ACP authentication failed: {error:?}", provider.label())
            })?;
    }
    Ok(())
}
