use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

pub(crate) const SERVICE_LISTEN_ENV: &str = "CLAUDEX_SERVICE_LISTEN";

pub(crate) fn parse_service_listen(raw: Option<&str>, bind: SocketAddr) -> SocketAddr {
    raw.and_then(|value| value.parse().ok()).unwrap_or(bind)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HandoverCommand {
    None,
    Ephemeral,
    Bind(SocketAddr),
}

#[derive(Clone, Debug)]
pub(crate) struct ListenHandover {
    request: watch::Sender<HandoverCommand>,
    advertised: Arc<RwLock<SocketAddr>>,
    cache: PathBuf,
    canonical: SocketAddr,
    service: SocketAddr,
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct RebindState {
    pub listen: SocketAddr,
    pub pid: u32,
}

impl ListenHandover {
    #[cfg(test)]
    pub(crate) fn new(
        canonical: SocketAddr,
        cache: PathBuf,
    ) -> (Self, watch::Receiver<HandoverCommand>) {
        Self::new_with_service(canonical, canonical, cache)
    }

    pub(crate) fn from_runtime_bind(
        bind: SocketAddr,
        cache: PathBuf,
    ) -> (Self, watch::Receiver<HandoverCommand>) {
        let service = parse_service_listen(std::env::var(SERVICE_LISTEN_ENV).ok().as_deref(), bind);
        Self::new_with_service(bind, service, cache)
    }

    pub(crate) fn new_with_service(
        initial: SocketAddr,
        service: SocketAddr,
        cache: PathBuf,
    ) -> (Self, watch::Receiver<HandoverCommand>) {
        let (request, rx) = watch::channel(HandoverCommand::None);
        (
            Self {
                request,
                advertised: Arc::new(RwLock::new(initial)),
                cache,
                canonical: initial,
                service,
            },
            rx,
        )
    }

    pub(crate) fn advertised_addr(&self) -> SocketAddr {
        *self.advertised.read().expect("listen handover lock")
    }

    #[cfg(test)]
    pub(crate) fn canonical_addr(&self) -> SocketAddr {
        self.canonical
    }

    pub(crate) fn service_addr(&self) -> SocketAddr {
        self.service
    }

    #[cfg(test)]
    pub(crate) fn set_advertised_for_test(&self, listen: SocketAddr) {
        *self.advertised.write().expect("listen handover lock") = listen;
    }

    pub(crate) fn request_ephemeral(&self) {
        let _ = self.request.send(HandoverCommand::Ephemeral);
    }

    pub(crate) fn request_bind(&self, listen: SocketAddr) {
        let _ = self.request.send(HandoverCommand::Bind(listen));
    }
}

pub(crate) struct HandoverListener {
    listener: TcpListener,
    advertised: Arc<RwLock<SocketAddr>>,
    request: watch::Receiver<HandoverCommand>,
    cache: PathBuf,
    canonical: SocketAddr,
}

impl HandoverListener {
    pub(crate) fn new(
        listener: TcpListener,
        handover: &ListenHandover,
        request: watch::Receiver<HandoverCommand>,
    ) -> Self {
        Self {
            listener,
            advertised: Arc::clone(&handover.advertised),
            request,
            cache: handover.cache.clone(),
            canonical: handover.canonical,
        }
    }

    async fn apply(&mut self, command: HandoverCommand) -> Result<()> {
        let target = match command {
            HandoverCommand::None => return Ok(()),
            HandoverCommand::Ephemeral => ephemeral_bind_addr(self.canonical),
            HandoverCommand::Bind(listen) => listen,
        };
        let next = TcpListener::bind(target)
            .await
            .context("bind listener during handover")?;
        let listen = next.local_addr().context("read rebound listener")?;
        write_rebind_state(&self.cache, self.canonical, listen)?;
        self.listener = next;
        *self.advertised.write().expect("listen handover lock") = listen;
        tracing::info!(%listen, "adapter listener rebound");
        Ok(())
    }

    async fn apply_changed_request(
        &mut self,
        changed: Result<(), tokio::sync::watch::error::RecvError>,
    ) {
        if changed.is_err() {
            return;
        }
        let command = self.request.borrow().clone();
        if let Err(error) = self.apply(command).await {
            tracing::error!(%error, "listener handover failed");
        }
    }
}

impl axum::serve::Listener for HandoverListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                biased;
                changed = self.request.changed() => {
                    self.apply_changed_request(changed).await;
                }
                result = self.listener.accept() => {
                    if let Some(accepted) = accept_or_backoff(result).await {
                        return accepted;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(*self.advertised.read().expect("listen handover lock"))
    }
}

#[path = "listen_handover_support.rs"]
mod support;
use support::{accept_or_backoff, ephemeral_bind_addr, write_rebind_state};
pub(crate) use support::rebind_state_path;


#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "listen_handover_tests.rs"]
mod tests;
