use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
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

async fn accept_or_backoff(
    result: std::io::Result<(TcpStream, SocketAddr)>,
) -> Option<(TcpStream, SocketAddr)> {
    match result {
        Ok(accepted) => Some(accepted),
        Err(error) => {
            tracing::error!(%error, "adapter accept failed");
            tokio::time::sleep(Duration::from_millis(10)).await;
            None
        }
    }
}

fn ephemeral_bind_addr(canonical: SocketAddr) -> SocketAddr {
    match canonical.ip() {
        std::net::IpAddr::V4(_) => SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0),
        std::net::IpAddr::V6(_) => SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), 0),
    }
}

fn write_rebind_state(cache: &Path, canonical: SocketAddr, listen: SocketAddr) -> Result<()> {
    fs::create_dir_all(cache).context("create rebind state directory")?;
    let path = rebind_state_path(cache, &canonical);
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create rebind state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure rebind state")?;
    }
    output
        .write_all(
            &serde_json::to_vec(&RebindState {
                listen,
                pid: std::process::id(),
            })
            .context("encode rebind state")?,
        )
        .context("write rebind state")?;
    output.sync_all().context("sync rebind state")?;
    fs::rename(&temporary, path).context("publish rebind state")
}

pub(crate) fn rebind_state_path(cache: &Path, canonical: &SocketAddr) -> PathBuf {
    let token: String = canonical
        .to_string()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    cache.join(format!("rebind.{token}.json"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "listen_handover_tests.rs"]
mod tests;
